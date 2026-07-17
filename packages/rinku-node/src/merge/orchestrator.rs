use anyhow::Result;
use rinku_core::types::{Account, Checkpoint, DagNode, SignedTransaction};
use rinku_core::weight::calculate_account_weight;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use tracing::info;

use super::{
    cascade, conflict_detection, resolution, MergePhase, MergeReport, MergeRequest,
    PartitionTxSummary, PenaltyAssessment, ViolationType,
};
use crate::events::EventBus;
use crate::state::NodeState;

const NONCE_REUSE_BALANCE_PENALTY_PCT: f64 = 0.10;
const NONCE_REUSE_STAKE_SLASH_PCT: f64 = 1.0;
const NONCE_REUSE_REPUTATION_PENALTY: f64 = 0.50;
const OVERDRAFT_REPUTATION_PENALTY: f64 = 0.10;
const OVERDRAFT_DECAY_CHECKPOINTS: u64 = 100;

/// Resolve remote tx weight: prefer peer-reported DAG weight, else account weight at tx time.
pub(crate) fn resolve_remote_tx_weight(remote: &MergeRequest, tx: &SignedTransaction) -> f64 {
    if let Some(&w) = remote.tx_weights.get(&tx.hash) {
        return w;
    }
    remote
        .accounts
        .get(&tx.tx.from)
        .map(|a| {
            // timestamps are usually ms; age weight expects seconds
            let ts_secs = if tx.tx.timestamp > 4_000_000_000 {
                tx.tx.timestamp / 1000
            } else {
                tx.tx.timestamp
            };
            calculate_account_weight(a, ts_secs)
        })
        .unwrap_or(1.0)
}

pub struct MergeOrchestrator {
    state: NodeState,
    event_bus: Option<std::sync::Arc<EventBus>>,
}

impl MergeOrchestrator {
    pub fn new(state: NodeState) -> Self {
        Self {
            state,
            event_bus: None,
        }
    }

    pub fn with_event_bus(mut self, event_bus: std::sync::Arc<EventBus>) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    fn emit(&self, event: crate::events::NodeEvent) {
        if let Some(ref eb) = self.event_bus {
            eb.publish(event);
        }
    }

    pub async fn prepare_merge_payload(
        &self,
        fork_point_checkpoint_height: u64,
    ) -> Result<MergeRequest> {
        let state = self.state.inner.read().await;

        let partition_epoch = state.partition_state.current_epoch.unwrap_or(0);

        let mut tx_weights = HashMap::new();
        let transactions: Vec<SignedTransaction> = state
            .dag
            .get_all_nodes()
            .into_iter()
            .filter(|node| match node.checkpoint_height {
                Some(h) => h > fork_point_checkpoint_height,
                None => true,
            })
            .map(|node| {
                tx_weights.insert(node.hash.clone(), node.weight);
                node.tx.clone()
            })
            .collect();

        let accounts = state.accounts.clone();

        let checkpoints: Vec<_> = state
            .checkpoints
            .iter()
            .filter(|cp| cp.height > fork_point_checkpoint_height)
            .cloned()
            .collect();

        let visible_stake_pct = state.partition_state.visible_stake_pct;

        Ok(MergeRequest {
            partition_epoch,
            fork_point_checkpoint_height,
            transactions,
            tx_weights,
            accounts,
            checkpoints,
            visible_stake_pct,
        })
    }

    pub async fn execute_merge(&self, remote: MergeRequest) -> Result<MergeReport> {
        let fork_point = remote.fork_point_checkpoint_height;
        let merge_epoch = remote.partition_epoch;

        let mut report = MergeReport::new(merge_epoch, fork_point);

        info!(
            "Starting merge: epoch={}, fork_point_checkpoint={}, remote_txs={}",
            merge_epoch,
            fork_point,
            remote.transactions.len()
        );

        report.phase = MergePhase::DagExchange;
        self.emit(crate::events::NodeEvent::MergeProgress {
            phase: "DagExchange".into(),
            detail: format!(
                "Exchanging DAG with {} remote txs",
                remote.transactions.len()
            ),
        });

        let local_txs = {
            let state = self.state.inner.read().await;
            let nodes: Vec<_> = state
                .dag
                .get_all_nodes()
                .into_iter()
                .filter(|node| match node.checkpoint_height {
                    Some(h) => h > fork_point,
                    None => true,
                })
                .cloned()
                .collect();
            nodes
        };

        let local_visible_stake = {
            let state = self.state.inner.read().await;
            state.partition_state.visible_stake_pct
        };

        let local_summaries: Vec<PartitionTxSummary> = local_txs
            .iter()
            .map(|node| {
                let mut summary = PartitionTxSummary::from_signed_tx(&node.tx, node.weight);
                summary.partition_epoch = node.partition_epoch;
                summary.visible_stake_pct = local_visible_stake;
                summary
            })
            .collect();

        let remote_summaries: Vec<PartitionTxSummary> = remote
            .transactions
            .iter()
            .map(|tx| {
                let weight = resolve_remote_tx_weight(&remote, tx);
                let mut summary = PartitionTxSummary::from_signed_tx(tx, weight);
                summary.partition_epoch = Some(remote.partition_epoch);
                summary.visible_stake_pct = remote.visible_stake_pct;
                summary
            })
            .collect();

        report.local_tx_count = local_summaries.len();
        report.remote_tx_count = remote_summaries.len();

        info!(
            "Merge DAG exchange: {} local txs, {} remote txs",
            local_summaries.len(),
            remote_summaries.len()
        );

        report.phase = MergePhase::ConflictDetection;
        self.emit(crate::events::NodeEvent::MergeProgress {
            phase: "ConflictDetection".into(),
            detail: format!(
                "{} local txs, {} remote txs",
                local_summaries.len(),
                remote_summaries.len()
            ),
        });

        let fork_point_accounts = self.get_fork_point_accounts(fork_point).await;

        let (direct_conflicts, economic_conflicts) = conflict_detection::detect_all_conflicts(
            &local_summaries,
            &remote_summaries,
            &fork_point_accounts,
        );

        info!(
            "Merge conflict detection: {} direct conflicts, {} economic conflicts",
            direct_conflicts.len(),
            economic_conflicts.len()
        );

        report.direct_conflicts = direct_conflicts.clone();
        report.economic_conflicts = economic_conflicts.clone();

        report.phase = MergePhase::WeightResolution;
        self.emit(crate::events::NodeEvent::MergeProgress {
            phase: "WeightResolution".into(),
            detail: format!(
                "{} direct, {} economic conflicts",
                direct_conflicts.len(),
                economic_conflicts.len()
            ),
        });

        let resolutions = resolution::resolve_all_conflicts(
            &direct_conflicts,
            &economic_conflicts,
            &local_summaries,
            &remote_summaries,
            &fork_point_accounts,
        );

        info!("Merge weight resolution: {} resolutions", resolutions.len());

        let mut conflict_losers: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for res in &resolutions {
            conflict_losers.extend(res.loser_tx_hashes.clone());
        }

        report.resolutions = resolutions;

        report.phase = MergePhase::CascadeRollback;
        self.emit(crate::events::NodeEvent::MergeProgress {
            phase: "CascadeRollback".into(),
            detail: format!("{} conflict losers to cascade", conflict_losers.len()),
        });

        let all_txs: Vec<PartitionTxSummary> = local_summaries
            .iter()
            .chain(remote_summaries.iter())
            .cloned()
            .collect();

        let fork_balances_micro = cascade::build_fork_point_balances_micro(&fork_point_accounts);
        let fork_nonces = cascade::build_fork_point_nonces(&fork_point_accounts);

        let rollback_report = cascade::cascade_rollback(
            &conflict_losers,
            &all_txs,
            &fork_balances_micro,
            &fork_nonces,
        );

        info!(
            "Merge cascade rollback: {} direct losers, {} cascade rollbacks, {} surviving",
            rollback_report.direct_conflict_losers.len(),
            rollback_report.cascade_rollbacks.len(),
            rollback_report.surviving_tx_hashes.len(),
        );

        let mut all_rejected: Vec<String> = conflict_losers.iter().cloned().collect();
        for cr in &rollback_report.cascade_rollbacks {
            all_rejected.push(cr.tx_hash.clone());
        }
        all_rejected.sort();
        all_rejected.dedup();

        report.cascade_rollbacks = rollback_report.cascade_rollbacks;
        report.cascade_rejected_count = report.cascade_rollbacks.len();
        report.final_balances_micro = Some(rollback_report.final_balances_micro.clone());
        report.transactions_kept = rollback_report.surviving_tx_hashes;
        report.transactions_rejected = all_rejected;

        for hash in &report.transactions_rejected {
            let reason = if conflict_losers.contains(hash) {
                "DirectConflictLoser"
            } else {
                "CascadeRollback"
            };
            self.emit(crate::events::NodeEvent::TransactionRolledBack {
                tx_hash: hash.clone(),
                reason: reason.into(),
            });
        }

        let penalties = self.assess_penalties(&report, &fork_point_accounts);
        for penalty in &penalties {
            self.emit(crate::events::NodeEvent::PenaltyAssessed {
                account: penalty.account.clone(),
                violation_type: format!("{:?}", penalty.violation_type),
                amount: penalty.reputation_penalty,
            });
        }
        report.penalties = penalties;

        info!("Merge penalties: {} assessed", report.penalties.len());

        report.phase = MergePhase::StateReconciliation;
        self.emit(crate::events::NodeEvent::MergeProgress {
            phase: "StateReconciliation".into(),
            detail: format!(
                "{} kept, {} rejected, {} penalties",
                report.transactions_kept.len(),
                report.transactions_rejected.len(),
                report.penalties.len()
            ),
        });

        self.apply_merge_state(
            &report,
            &remote,
            &rollback_report.final_balances_micro,
            &report.transactions_kept.clone(),
            fork_point,
        )
        .await;

        report.complete();

        info!(
            "Merge complete: {} kept, {} rejected ({} cascaded) (duration: {}ms)",
            report.transactions_kept.len(),
            report.transactions_rejected.len(),
            report.cascade_rejected_count,
            report.completed_at_ms.unwrap_or(0) - report.started_at_ms
        );

        Ok(report)
    }

    async fn apply_merge_state(
        &self,
        report: &MergeReport,
        remote_request: &MergeRequest,
        final_balances_micro: &HashMap<String, u64>,
        surviving_tx_hashes: &[String],
        fork_point_checkpoint_height: u64,
    ) {
        let direct_losers: HashSet<&str> = report
            .resolutions
            .iter()
            .flat_map(|res| res.loser_tx_hashes.iter())
            .map(|s| s.as_str())
            .collect();

        let cascade_hashes: HashSet<&str> = report
            .cascade_rollbacks
            .iter()
            .map(|cr| cr.tx_hash.as_str())
            .collect();

        let rejected_set: HashSet<&str> = report
            .transactions_rejected
            .iter()
            .map(|s| s.as_str())
            .collect();

        let surviving_set: HashSet<&str> = surviving_tx_hashes.iter().map(|s| s.as_str()).collect();

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let now_secs = now_ms / 1000;

        let mut state = self.state.inner.write().await;

        for (addr, &balance_micro) in final_balances_micro {
            if let Some(account) = state.accounts.get_mut(addr) {
                account.balance = balance_micro;
            } else if balance_micro > 0 {
                state.accounts.insert(
                    addr.clone(),
                    Account {
                        address: addr.clone(),
                        balance: balance_micro,
                        nonce: 0,
                        first_seen: now_secs,
                        staked: 0,
                        unbonding: 0,
                        unbonding_release: None,
                        latest_balance_proof: None,
                        partition_violations: 0,
                        reputation_penalty: 0.0,
                        penalty_decay_checkpoint: None,
                        partition_budget: None,
                        partition_budget_spent: 0,
                        ecdsa_public_key: None,
                    },
                );
            }
        }

        let mut nonce_counts: HashMap<String, u64> = HashMap::new();
        for hash in surviving_tx_hashes {
            if let Some(node) = state.dag.get_node(hash) {
                *nonce_counts.entry(node.tx.tx.from.clone()).or_insert(0) += 1;
            } else if let Some(remote_tx) =
                remote_request.transactions.iter().find(|t| t.hash == *hash)
            {
                *nonce_counts.entry(remote_tx.tx.from.clone()).or_insert(0) += 1;
            }
        }

        for remote_tx in &remote_request.transactions {
            if surviving_set.contains(remote_tx.hash.as_str())
                && !state.dag.contains(&remote_tx.hash)
            {
                let weight = resolve_remote_tx_weight(remote_request, remote_tx);
                let node = DagNode {
                    hash: remote_tx.hash.clone(),
                    parents: remote_tx.tx.parents.clone(),
                    children: vec![],
                    weight,
                    finalized: true,
                    checkpoint_height: Some(fork_point_checkpoint_height + 1),
                    tx: remote_tx.clone(),
                    received_at_ms: Some(now_ms),
                    partition_epoch: Some(report.merge_epoch),
                    rolled_back: false,
                    fast_path_cert: None,
                };
                let _ = state.dag.add_node(node);
            }
        }

        let mut removed_count = 0u64;
        let mut marked_count = 0u64;

        let all_hashes: Vec<String> = state
            .dag
            .get_all_nodes()
            .iter()
            .map(|n| n.hash.clone())
            .collect();

        for hash in &all_hashes {
            if !rejected_set.contains(hash.as_str()) {
                continue;
            }

            let is_direct_loser = direct_losers.contains(hash.as_str());

            if is_direct_loser {
                state.dag.remove_node(hash);
                removed_count += 1;
            } else if cascade_hashes.contains(hash.as_str()) {
                state.dag.unfinalize(hash);
                if let Some(node) = state.dag.get_node_mut(hash) {
                    node.rolled_back = true;
                }
                marked_count += 1;
            }
        }

        for hash in surviving_tx_hashes {
            let _ = state
                .dag
                .mark_finalized(hash, fork_point_checkpoint_height + 1);
            if let Some(node) = state.dag.get_node_mut(hash) {
                node.rolled_back = false;
            }
        }

        let merge_height = fork_point_checkpoint_height + 1;
        for penalty in &report.penalties {
            let slash_amount = if let Some(account) = state.accounts.get_mut(&penalty.account) {
                if penalty.balance_penalty_micro > 0 {
                    account.balance = account
                        .balance
                        .saturating_sub(penalty.balance_penalty_micro);
                }
                let slash = if penalty.stake_slash_pct > 0.0 {
                    let s = (account.staked as f64 * penalty.stake_slash_pct).round() as u64;
                    account.staked = account.staked.saturating_sub(s);
                    s
                } else {
                    0
                };
                account.reputation_penalty =
                    (account.reputation_penalty + penalty.reputation_penalty).min(1.0);
                account.partition_violations += 1;
                if let Some(decay) = penalty.decay_checkpoints {
                    account.penalty_decay_checkpoint = Some(merge_height + decay);
                }
                slash
            } else {
                0
            };
            if slash_amount > 0 {
                if let Some(validator) = state.validators.get_mut(&penalty.account) {
                    validator.stake = validator.stake.saturating_sub(slash_amount);
                }
            }
        }

        let provisional_count = state
            .checkpoints
            .iter()
            .filter(|cp| cp.height > fork_point_checkpoint_height && cp.provisional)
            .count();
        state
            .checkpoints
            .retain(|cp| cp.height <= fork_point_checkpoint_height || !cp.provisional);

        let merge_report_hash = {
            let report_json = serde_json::to_string(report).unwrap_or_default();
            let mut hasher = Sha256::new();
            hasher.update(report_json.as_bytes());
            hex::encode(hasher.finalize())
        };

        let fork_point_hash = state
            .checkpoints
            .iter()
            .find(|cp| cp.height == fork_point_checkpoint_height)
            .map(|cp| cp.hash.clone());

        let tx_merkle_root = {
            let mut sorted_hashes = surviving_tx_hashes.to_vec();
            sorted_hashes.sort();
            let concatenated = sorted_hashes.join(",");
            let mut hasher = Sha256::new();
            hasher.update(concatenated.as_bytes());
            hex::encode(hasher.finalize())
        };

        let state_root = {
            let mut account_data: Vec<String> = state
                .accounts
                .iter()
                .map(|(addr, acct)| format!("{}:{:.8}:{}", addr, acct.balance, acct.nonce))
                .collect();
            account_data.sort();
            let concatenated = account_data.join(",");
            let mut hasher = Sha256::new();
            hasher.update(concatenated.as_bytes());
            hex::encode(hasher.finalize())
        };

        let tip_count = state.dag.tip_count() as u32;

        let cp_hash_data = format!(
            "{}:{}:{}:{}:{}:{}",
            merge_height, tx_merkle_root, state_root, "", tip_count, now_secs
        );
        let checkpoint_hash = {
            let mut hasher = Sha256::new();
            hasher.update(cp_hash_data.as_bytes());
            hex::encode(hasher.finalize())
        };

        let merge_checkpoint = Checkpoint {
            height: merge_height,
            hash: checkpoint_hash,
            previous_hash: fork_point_hash,
            tx_merkle_root,
            state_root,
            receipt_root: String::new(),
            tip_count,
            timestamp: now_secs,
            validator_signatures: Vec::new(),
            aggregated_signature: None,
            signer_bitmap: None,
            finalized_tx_hashes: surviving_tx_hashes.to_vec(),
            weight_trie_root: String::new(),
            provisional: false,
            partition_epoch: Some(report.merge_epoch),
            visible_stake_pct: Some(1.0),
            merge_report_hash: Some(merge_report_hash),
            view_change_certificate: None,
            view: 0,
        };

        state.checkpoints.push(merge_checkpoint);
        let merge_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
        self.state
            .checkpoint_height_cache
            .store(merge_height, std::sync::atomic::Ordering::Relaxed);

        info!(
            "State reconciliation complete: {} balances updated, {} remote txs ingested, \
             {} direct-conflict txs removed, {} cascade txs marked rolled_back, \
             {} provisional checkpoints retired, merge checkpoint at height {}",
            final_balances_micro.len(),
            remote_request
                .transactions
                .iter()
                .filter(|t| surviving_set.contains(t.hash.as_str()))
                .count(),
            removed_count,
            marked_count,
            provisional_count,
            merge_height,
        );
    }

    async fn get_fork_point_accounts(
        &self,
        fork_point_checkpoint_height: u64,
    ) -> HashMap<String, Account> {
        let state = self.state.inner.read().await;

        if fork_point_checkpoint_height == 0 {
            return state.accounts.clone();
        }

        state.accounts.clone()
    }

    fn assess_penalties(
        &self,
        report: &MergeReport,
        fork_point_accounts: &HashMap<String, Account>,
    ) -> Vec<PenaltyAssessment> {
        let mut penalties: Vec<PenaltyAssessment> = Vec::new();
        let mut penalized_accounts: HashSet<String> = HashSet::new();

        for conflict in &report.direct_conflicts {
            if penalized_accounts.contains(&conflict.account) {
                continue;
            }
            penalized_accounts.insert(conflict.account.clone());

            let balance_micro = fork_point_accounts
                .get(&conflict.account)
                .map(|a| a.balance)
                .unwrap_or(0);
            let penalty_micro = balance_micro / 10;

            penalties.push(PenaltyAssessment {
                account: conflict.account.clone(),
                violation_type: ViolationType::NonceReuse,
                balance_penalty_micro: penalty_micro,
                stake_slash_pct: NONCE_REUSE_STAKE_SLASH_PCT,
                reputation_penalty: NONCE_REUSE_REPUTATION_PENALTY,
                decay_checkpoints: None,
                conflicting_tx_hashes: vec![
                    conflict.local_tx_hash.clone(),
                    conflict.remote_tx_hash.clone(),
                ],
            });
        }

        for conflict in &report.economic_conflicts {
            if penalized_accounts.contains(&conflict.account) {
                continue;
            }

            penalties.push(PenaltyAssessment {
                account: conflict.account.clone(),
                violation_type: ViolationType::CrossPartitionOverdraft,
                balance_penalty_micro: 0,
                stake_slash_pct: 0.0,
                reputation_penalty: OVERDRAFT_REPUTATION_PENALTY,
                decay_checkpoints: Some(OVERDRAFT_DECAY_CHECKPOINTS),
                conflicting_tx_hashes: conflict
                    .local_tx_hashes
                    .iter()
                    .chain(conflict.remote_tx_hashes.iter())
                    .cloned()
                    .collect(),
            });
        }

        penalties
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rinku_core::types::{to_micro_units, Account, SignedTransaction, Transaction};

    fn make_account(addr: &str, balance: f64, staked: f64) -> Account {
        Account {
            address: addr.to_string(),
            balance: to_micro_units(balance),
            nonce: 0,
            first_seen: 1_700_000_000,
            staked: to_micro_units(staked),
            unbonding: 0,
            unbonding_release: None,
            latest_balance_proof: None,
            partition_violations: 0,
            reputation_penalty: 0.0,
            penalty_decay_checkpoint: None,
            partition_budget: None,
            partition_budget_spent: 0,
            ecdsa_public_key: None,
        }
    }

    fn make_signed(hash: &str, from: &str) -> SignedTransaction {
        SignedTransaction {
            tx: Transaction {
                from: from.to_string(),
                to: "bob".to_string(),
                amount: to_micro_units(1.0),
                nonce: 0,
                timestamp: 1_700_000_000_000,
                parents: vec![],
                kind: None,
                gas_limit: None,
                gas_price: None,
                data: None,
                signature: None,
                memo: None,
                references: None,
            },
            hash: hash.to_string(),
            signature: "sig".to_string(),
        }
    }

    fn empty_request(accounts: HashMap<String, Account>) -> MergeRequest {
        MergeRequest {
            partition_epoch: 1,
            fork_point_checkpoint_height: 0,
            transactions: vec![],
            tx_weights: HashMap::new(),
            accounts,
            checkpoints: vec![],
            visible_stake_pct: 0.5,
        }
    }

    #[test]
    fn resolve_remote_prefers_tx_weights_map() {
        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), make_account("alice", 100.0, 0.0));
        let mut remote = empty_request(accounts);
        let tx = make_signed("h1", "alice");
        remote.tx_weights.insert("h1".to_string(), 42.5);
        assert!((resolve_remote_tx_weight(&remote, &tx) - 42.5).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_remote_falls_back_to_account_weight() {
        let mut accounts = HashMap::new();
        // Staked account yields weight > 1.0 via stake term
        accounts.insert("alice".to_string(), make_account("alice", 100.0, 1_000.0));
        let remote = empty_request(accounts);
        let tx = make_signed("h2", "alice");
        let w = resolve_remote_tx_weight(&remote, &tx);
        assert!(w > 1.0, "expected account-derived weight > 1.0, got {w}");
    }

    #[test]
    fn resolve_remote_defaults_to_one_without_account() {
        let remote = empty_request(HashMap::new());
        let tx = make_signed("h3", "unknown");
        assert!((resolve_remote_tx_weight(&remote, &tx) - 1.0).abs() < f64::EPSILON);
    }
}
