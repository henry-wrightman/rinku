use serde::Serialize;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum NodeEvent {
    NewTransaction {
        hash: String,
        from: String,
        to: String,
        amount: f64,
        kind: Option<String>,
    },
    FastPathConfirmed {
        hash: String,
        from: String,
        to: String,
        amount: f64,
        total_stake: f64,
        threshold: f64,
    },
    FastPathExecuted {
        hash: String,
        from: String,
        to: String,
        amount: f64,
    },
    CheckpointCreated {
        hash: String,
        height: u64,
        txs_finalized: usize,
        reward: f64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        validator_rewards: Vec<(String, f64)>,
    },
    AccountUpdated {
        address: String,
        balance: f64,
        nonce: u64,
        staked: f64,
    },
    PartitionSuspected {
        visible_stake_pct: f64,
        missing_validators: Vec<String>,
    },
    PartitionConfirmed {
        epoch: u64,
        visible_validators: Vec<String>,
    },
    PartitionHealed {
        visible_validators: Vec<String>,
    },
    MergeStarted {
        epoch: u64,
        fork_point_checkpoint: u64,
        remote_tx_count: usize,
    },
    MergeCompleted {
        epoch: u64,
        direct_conflicts: usize,
        economic_conflicts: usize,
        transactions_kept: usize,
        transactions_rejected: usize,
        duration_ms: u64,
    },
    MergeProgress {
        phase: String,
        detail: String,
    },
    TransactionRolledBack {
        tx_hash: String,
        reason: String,
    },
    PenaltyAssessed {
        account: String,
        violation_type: String,
        amount: f64,
    },
}

#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<NodeEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn publish(&self, event: NodeEvent) {
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<NodeEvent> {
        self.sender.subscribe()
    }
}
