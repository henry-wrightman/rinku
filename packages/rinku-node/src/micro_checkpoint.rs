use rinku_core::types::{Account, MicroCheckpoint, WriteSet};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

const MICRO_CHECKPOINT_INTERVAL_MS: u64 = 200;
const MAX_MICRO_CHECKPOINTS: usize = 64;

#[derive(Debug, Clone)]
pub struct PendingWriteSet {
    pub tx_hash: String,
    pub write_set_hash: String,
    pub changed_accounts: Vec<(String, Account)>,
}

pub struct MicroCheckpointServiceInner {
    sequence: u64,
    checkpoints: Vec<MicroCheckpoint>,
    pending_write_sets: Vec<PendingWriteSet>,
    parent_qcc_height: u64,
    tx_to_micro_cp: HashMap<String, u64>,
}

pub struct MicroCheckpointService {
    inner: Arc<RwLock<MicroCheckpointServiceInner>>,
}

impl Clone for MicroCheckpointService {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl MicroCheckpointService {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(MicroCheckpointServiceInner {
                sequence: 0,
                checkpoints: Vec::new(),
                pending_write_sets: Vec::new(),
                parent_qcc_height: 0,
                tx_to_micro_cp: HashMap::new(),
            })),
        }
    }

    pub async fn add_pending_write_set(&self, ws: PendingWriteSet) {
        let mut inner = self.inner.write().await;
        inner.pending_write_sets.push(ws);
    }

    pub async fn set_parent_qcc_height(&self, height: u64) {
        let mut inner = self.inner.write().await;
        inner.parent_qcc_height = height;
    }

    pub async fn produce_micro_checkpoint(
        &self,
        state: &crate::state::NodeState,
    ) -> Option<MicroCheckpoint> {
        let (pending, parent_height, seq) = {
            let mut inner = self.inner.write().await;
            if inner.pending_write_sets.is_empty() {
                return None;
            }
            inner.sequence += 1;
            let seq = inner.sequence;
            let pending = std::mem::take(&mut inner.pending_write_sets);
            (pending, inner.parent_qcc_height, seq)
        };

        let mut all_changed: HashMap<String, Account> = HashMap::new();
        let mut tx_hashes: Vec<String> = Vec::new();

        for ws in &pending {
            tx_hashes.push(ws.tx_hash.clone());
            for (addr, acc) in &ws.changed_accounts {
                all_changed.insert(addr.clone(), acc.clone());
            }
        }

        let changed_addresses: Vec<String> = all_changed.keys().cloned().collect();

        let state_root = {
            if !changed_addresses.is_empty() {
                let trie_updates = {
                    let state_inner = state.inner.read().await;
                    state_inner.collect_trie_updates_for_addresses(&changed_addresses)
                };
                state.update_trie_with_tuples(&trie_updates).await;
            }
            state.state_trie.lock().await.root_hex()
        };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let micro_cp = MicroCheckpoint {
            sequence: seq,
            state_root: state_root.clone(),
            timestamp_ms: now_ms,
            tx_hashes: tx_hashes.clone(),
            changed_accounts: changed_addresses.clone(),
            parent_qcc_height: parent_height,
        };

        {
            let mut inner = self.inner.write().await;
            for tx_hash in &tx_hashes {
                inner.tx_to_micro_cp.insert(tx_hash.clone(), seq);
            }
            inner.checkpoints.push(micro_cp.clone());
            if inner.checkpoints.len() > MAX_MICRO_CHECKPOINTS {
                let removed = inner.checkpoints.remove(0);
                for h in &removed.tx_hashes {
                    inner.tx_to_micro_cp.remove(h);
                }
            }
        }

        info!(
            "MICRO-CP #{}: {} txs, {} changed accounts, state_root={}..{}",
            seq,
            tx_hashes.len(),
            changed_addresses.len(),
            &state_root[..8.min(state_root.len())],
            &state_root[state_root.len().saturating_sub(4)..],
        );

        Some(micro_cp)
    }

    pub async fn get_micro_checkpoint_for_tx(&self, tx_hash: &str) -> Option<MicroCheckpoint> {
        let inner = self.inner.read().await;
        let seq = inner.tx_to_micro_cp.get(tx_hash)?;
        inner.checkpoints.iter().find(|cp| cp.sequence == *seq).cloned()
    }

    pub async fn get_latest(&self) -> Option<MicroCheckpoint> {
        let inner = self.inner.read().await;
        inner.checkpoints.last().cloned()
    }

    pub async fn get_stats(&self) -> MicroCheckpointStats {
        let inner = self.inner.read().await;
        MicroCheckpointStats {
            current_sequence: inner.sequence,
            stored_checkpoints: inner.checkpoints.len(),
            pending_write_sets: inner.pending_write_sets.len(),
            tracked_txs: inner.tx_to_micro_cp.len(),
        }
    }

    pub async fn on_qcc_checkpoint(&self, height: u64) {
        let mut inner = self.inner.write().await;
        inner.parent_qcc_height = height;
        inner.checkpoints.retain(|cp| cp.parent_qcc_height >= height.saturating_sub(1));
        let remaining_seqs: std::collections::HashSet<u64> =
            inner.checkpoints.iter().map(|cp| cp.sequence).collect();
        inner.tx_to_micro_cp.retain(|_, seq| remaining_seqs.contains(seq));
    }

    pub async fn submit_write_sets(
        &self,
        write_sets: Vec<PendingWriteSet>,
        qcc_height: u64,
    ) {
        let mut inner = self.inner.write().await;
        inner.parent_qcc_height = qcc_height;
        for ws in write_sets {
            debug!(
                "Micro-CP: queued tx {} (ws_hash={}..)",
                &ws.tx_hash[..16.min(ws.tx_hash.len())],
                &ws.write_set_hash[..8.min(ws.write_set_hash.len())],
            );
            inner.pending_write_sets.push(ws);
        }
    }

    pub fn interval_ms() -> u64 {
        MICRO_CHECKPOINT_INTERVAL_MS
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicroCheckpointStats {
    pub current_sequence: u64,
    pub stored_checkpoints: usize,
    pub pending_write_sets: usize,
    pub tracked_txs: usize,
}
