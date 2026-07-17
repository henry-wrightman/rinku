use super::*;
use crate::merge::MergeReport;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PartitionStatus {
    Normal,
    Suspected,
    Partitioned,
}

impl Default for PartitionStatus {
    fn default() -> Self {
        PartitionStatus::Normal
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionState {
    pub status: PartitionStatus,
    pub current_epoch: Option<u64>,
    pub epoch_start_checkpoint: Option<u64>,
    pub epoch_start_timestamp: Option<u64>,
    pub visible_validators: Vec<String>,
    pub visible_stake_pct: f64,
    pub suspected_since: Option<u64>,
    pub total_epochs: u64,
    #[serde(skip)]
    pub latest_merge_report: Option<MergeReport>,
    #[serde(skip)]
    pub merge_history: Vec<MergeReport>,
}

impl Default for PartitionState {
    fn default() -> Self {
        Self {
            status: PartitionStatus::Normal,
            current_epoch: None,
            epoch_start_checkpoint: None,
            epoch_start_timestamp: None,
            visible_validators: Vec::new(),
            visible_stake_pct: 1.0,
            suspected_since: None,
            total_epochs: 0,
            latest_merge_report: None,
            merge_history: Vec::new(),
        }
    }
}

impl NodeState {
    pub async fn get_partition_state(&self) -> PartitionState {
        let state = self.inner.read().await;
        state.partition_state.clone()
    }

    pub async fn update_partition_visibility(
        &self,
        visible_validators: Vec<String>,
        visible_stake_pct: f64,
    ) {
        let mut state = self.inner.write().await;
        state.partition_state.visible_validators = visible_validators;
        state.partition_state.visible_stake_pct = visible_stake_pct;
    }

    pub async fn transition_to_suspected(&self) {
        let mut state = self.inner.write().await;
        if state.partition_state.status == PartitionStatus::Normal {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            state.partition_state.status = PartitionStatus::Suspected;
            state.partition_state.suspected_since = Some(now);
            info!(
                "Partition status: NORMAL -> SUSPECTED (visible stake: {:.1}%)",
                state.partition_state.visible_stake_pct * 100.0
            );
        }
    }

    pub async fn transition_to_partitioned(&self) -> u64 {
        let mut state = self.inner.write().await;
        if state.partition_state.status == PartitionStatus::Suspected {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let checkpoint_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
            state.partition_state.total_epochs += 1;
            let epoch = state.partition_state.total_epochs;
            state.partition_state.status = PartitionStatus::Partitioned;
            state.partition_state.current_epoch = Some(epoch);
            state.partition_state.epoch_start_checkpoint = Some(checkpoint_height);
            state.partition_state.epoch_start_timestamp = Some(now);
            state.partition_state.suspected_since = None;

            for account in state.accounts.values_mut() {
                account.partition_budget_spent = 0;
            }

            info!(
                "Partition status: SUSPECTED -> PARTITIONED (epoch: {}, fork point checkpoint: {}, visible stake: {:.1}%)",
                epoch, checkpoint_height, state.partition_state.visible_stake_pct * 100.0
            );
            epoch
        } else {
            state.partition_state.current_epoch.unwrap_or(0)
        }
    }

    pub async fn transition_to_normal(&self) {
        let mut state = self.inner.write().await;
        let prev_status = &state.partition_state.status;
        if *prev_status != PartitionStatus::Normal {
            let prev = format!("{:?}", prev_status);
            state.partition_state.status = PartitionStatus::Normal;
            state.partition_state.current_epoch = None;
            state.partition_state.epoch_start_checkpoint = None;
            state.partition_state.epoch_start_timestamp = None;
            state.partition_state.suspected_since = None;
            info!(
                "Partition status: {} -> NORMAL (visible stake: {:.1}%)",
                prev,
                state.partition_state.visible_stake_pct * 100.0
            );
        }
    }

    pub async fn get_current_partition_epoch(&self) -> Option<u64> {
        let state = self.inner.read().await;
        state.partition_state.current_epoch
    }

    pub async fn is_partitioned(&self) -> bool {
        let state = self.inner.read().await;
        state.partition_state.status == PartitionStatus::Partitioned
    }

    pub async fn set_latest_merge_report(&self, report: MergeReport) {
        let mut state = self.inner.write().await;
        state.partition_state.merge_history.push(report.clone());
        const MAX_MERGE_HISTORY: usize = 50;
        if state.partition_state.merge_history.len() > MAX_MERGE_HISTORY {
            state.partition_state.merge_history.remove(0);
        }
        state.partition_state.latest_merge_report = Some(report);
    }

    pub async fn get_latest_merge_report(&self) -> Option<MergeReport> {
        let state = self.inner.read().await;
        state.partition_state.latest_merge_report.clone()
    }

    pub async fn get_merge_report_by_epoch(&self, epoch: u64) -> Option<MergeReport> {
        let state = self.inner.read().await;
        state
            .partition_state
            .merge_history
            .iter()
            .find(|r| r.merge_epoch == epoch)
            .cloned()
    }

    pub async fn set_partition_budget(&self, address: &str, budget: Option<u64>) -> bool {
        let mut state = self.inner.write().await;
        if let Some(account) = state.accounts.get_mut(address) {
            account.partition_budget = budget;
            if budget.is_none() {
                account.partition_budget_spent = 0;
            }
            true
        } else {
            false
        }
    }

    pub async fn get_merge_history(&self) -> Vec<serde_json::Value> {
        let state = self.inner.read().await;
        state
            .partition_state
            .merge_history
            .iter()
            .map(|r| {
                serde_json::json!({
                    "merge_epoch": r.merge_epoch,
                    "fork_point_checkpoint_height": r.fork_point_checkpoint_height,
                    "phase": format!("{:?}", r.phase),
                    "direct_conflicts": r.direct_conflicts.len(),
                    "economic_conflicts": r.economic_conflicts.len(),
                    "transactions_kept": r.transactions_kept.len(),
                    "transactions_rejected": r.transactions_rejected.len(),
                    "penalties": r.penalties.len(),
                    "started_at_ms": r.started_at_ms,
                    "completed_at_ms": r.completed_at_ms,
                    "duration_ms": r.completed_at_ms.unwrap_or(0).saturating_sub(r.started_at_ms),
                })
            })
            .collect()
    }
}
