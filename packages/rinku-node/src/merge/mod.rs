pub mod cascade;
pub mod conflict_detection;
pub mod orchestrator;
#[cfg(test)]
mod proptests;
pub mod resolution;

use rinku_core::types::{Account, Checkpoint, SignedTransaction};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeRequest {
    pub partition_epoch: u64,
    pub fork_point_checkpoint_height: u64,
    pub transactions: Vec<SignedTransaction>,
    /// Peer-reported DAG node weights keyed by tx hash (avoids remote weight=1.0).
    #[serde(default)]
    pub tx_weights: HashMap<String, f64>,
    pub accounts: HashMap<String, Account>,
    pub checkpoints: Vec<Checkpoint>,
    pub visible_stake_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConflictType {
    DirectDoubleSpend,
    EconomicOverdraft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectConflict {
    pub account: String,
    pub nonce: u64,
    pub local_tx_hash: String,
    pub remote_tx_hash: String,
    pub local_partition_epoch: Option<u64>,
    pub remote_partition_epoch: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EconomicConflict {
    pub account: String,
    pub pre_partition_balance_micro: u64,
    pub total_sent_local_micro: u64,
    pub total_sent_remote_micro: u64,
    pub total_received_local_micro: u64,
    pub total_received_remote_micro: u64,
    pub deficit_micro: u64,
    pub local_tx_hashes: Vec<String>,
    pub remote_tx_hashes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ResolutionReason {
    HigherWeight,
    HigherStake,
    LowerHashTiebreak,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictResolution {
    pub conflict_type: ConflictType,
    pub account: String,
    pub winner_tx_hashes: Vec<String>,
    pub loser_tx_hashes: Vec<String>,
    pub reason: ResolutionReason,
    pub winner_weight: f64,
    pub loser_weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MergePhase {
    DagExchange,
    ConflictDetection,
    WeightResolution,
    CascadeRollback,
    StateReconciliation,
    Complete,
}

impl Default for MergePhase {
    fn default() -> Self {
        MergePhase::DagExchange
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ViolationType {
    NonceReuse,
    CrossPartitionOverdraft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PenaltyAssessment {
    pub account: String,
    pub violation_type: ViolationType,
    pub balance_penalty_micro: u64,
    pub stake_slash_pct: f64,
    pub reputation_penalty: f64,
    pub decay_checkpoints: Option<u64>,
    pub conflicting_tx_hashes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeReport {
    pub merge_epoch: u64,
    pub fork_point_checkpoint_height: u64,
    pub phase: MergePhase,
    pub direct_conflicts: Vec<DirectConflict>,
    pub economic_conflicts: Vec<EconomicConflict>,
    pub resolutions: Vec<ConflictResolution>,
    pub transactions_kept: Vec<String>,
    pub transactions_rejected: Vec<String>,
    pub cascade_rollbacks: Vec<CascadeRollback>,
    pub cascade_rejected_count: usize,
    pub final_balances_micro: Option<HashMap<String, u64>>,
    pub penalties: Vec<PenaltyAssessment>,
    pub local_tx_count: usize,
    pub remote_tx_count: usize,
    pub started_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub error: Option<String>,
}

impl MergeReport {
    pub fn new(merge_epoch: u64, fork_point_checkpoint_height: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            merge_epoch,
            fork_point_checkpoint_height,
            phase: MergePhase::DagExchange,
            direct_conflicts: Vec::new(),
            economic_conflicts: Vec::new(),
            resolutions: Vec::new(),
            transactions_kept: Vec::new(),
            transactions_rejected: Vec::new(),
            cascade_rollbacks: Vec::new(),
            cascade_rejected_count: 0,
            final_balances_micro: None,
            penalties: Vec::new(),
            local_tx_count: 0,
            remote_tx_count: 0,
            started_at_ms: now,
            completed_at_ms: None,
            error: None,
        }
    }

    pub fn complete(&mut self) {
        self.phase = MergePhase::Complete;
        self.completed_at_ms = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        );
    }

    pub fn fail(&mut self, error: String) {
        self.error = Some(error);
        self.completed_at_ms = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CascadeRollback {
    pub tx_hash: String,
    pub reason: RollbackReason,
    pub affected_account: String,
    pub amount_reverted_micro: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RollbackReason {
    DirectConflictLoser,
    InsufficientBalanceAfterConflictResolution,
    NonceContinuityGap,
    DependsOnRolledBackTransaction { upstream_tx: String },
}

#[derive(Debug, Clone)]
pub struct RollbackReport {
    pub direct_conflict_losers: std::collections::HashSet<String>,
    pub cascade_rollbacks: Vec<CascadeRollback>,
    pub final_balances_micro: HashMap<String, u64>,
    pub surviving_tx_hashes: Vec<String>,
    pub iterations: u32,
}

#[derive(Debug, Clone)]
pub struct PartitionTxSummary {
    pub tx_hash: String,
    pub from: String,
    pub to: String,
    pub amount_micro: u64,
    pub gas_micro: u64,
    pub nonce: u64,
    pub weight: f64,
    /// Topological distance from fork-point within the partition DAG set.
    pub dag_depth: u32,
    pub parents: Vec<String>,
    pub partition_epoch: Option<u64>,
    pub visible_stake_pct: f64,
}

impl PartitionTxSummary {
    pub fn from_signed_tx(tx: &SignedTransaction, weight: f64) -> Self {
        let gas_micro = tx.tx.gas_price.unwrap_or(0) * tx.tx.gas_limit.unwrap_or(0);
        Self {
            tx_hash: tx.hash.clone(),
            from: tx.tx.from.clone(),
            to: tx.tx.to.clone(),
            amount_micro: tx.tx.amount,
            gas_micro,
            nonce: tx.tx.nonce,
            weight,
            dag_depth: 0,
            parents: tx.tx.parents.clone(),
            partition_epoch: None,
            visible_stake_pct: 0.0,
        }
    }
}
