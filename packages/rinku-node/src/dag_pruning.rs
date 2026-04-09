use crate::storage::{DagSnapshotEntry, RedbStorage};
use anyhow::Result;
use rinku_core::types::SignedTransaction;
use std::collections::{HashSet, VecDeque};
use tracing::{debug, info};

pub const DEFAULT_RETENTION_CHECKPOINTS: u64 = 50;
pub const DEFAULT_MAX_DAG_NODES: usize = 100_000;
pub const PRUNING_BATCH_SIZE: usize = 1000;

#[derive(Debug, Clone)]
pub struct PruningConfig {
    pub retention_checkpoints: u64,
    pub max_dag_nodes: usize,
    pub prune_interval_secs: u64,
}

impl Default for PruningConfig {
    fn default() -> Self {
        Self {
            retention_checkpoints: DEFAULT_RETENTION_CHECKPOINTS,
            max_dag_nodes: DEFAULT_MAX_DAG_NODES,
            prune_interval_secs: 300,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PruningStats {
    pub last_prune_time: u64,
    pub nodes_pruned: u64,
    pub checkpoints_pruned: u64,
    pub bytes_freed: u64,
    pub oldest_retained_checkpoint: u64,
}

impl Default for PruningStats {
    fn default() -> Self {
        Self {
            last_prune_time: 0,
            nodes_pruned: 0,
            checkpoints_pruned: 0,
            bytes_freed: 0,
            oldest_retained_checkpoint: 0,
        }
    }
}

pub struct DagPruningService {
    config: PruningConfig,
    stats: PruningStats,
}

impl DagPruningService {
    pub fn new(config: PruningConfig) -> Self {
        info!(
            "DAG pruning service initialized: retain {} checkpoints, max {} nodes",
            config.retention_checkpoints, config.max_dag_nodes
        );
        Self {
            config,
            stats: PruningStats::default(),
        }
    }

    pub fn should_prune(&self, current_checkpoint: u64) -> bool {
        current_checkpoint > self.config.retention_checkpoints
    }

    pub fn calculate_prune_boundary(&self, current_checkpoint: u64) -> u64 {
        if current_checkpoint <= self.config.retention_checkpoints {
            0
        } else {
            current_checkpoint - self.config.retention_checkpoints
        }
    }

    pub fn prune_dag(
        &mut self,
        storage: &RedbStorage,
        current_checkpoint: u64,
        finalized_tx_hashes: &HashSet<String>,
    ) -> Result<PruningStats> {
        let prune_boundary = self.calculate_prune_boundary(current_checkpoint);
        if prune_boundary == 0 {
            debug!("No pruning needed, checkpoint {} below threshold", current_checkpoint);
            return Ok(self.stats.clone());
        }

        info!(
            "Starting DAG prune: checkpoint={}, boundary={}, retaining {} checkpoints",
            current_checkpoint, prune_boundary, self.config.retention_checkpoints
        );

        let mut nodes_pruned = 0u64;
        let mut checkpoints_pruned = 0u64;
        let mut keys_to_delete: Vec<Vec<u8>> = Vec::new();

        storage.iter_dag::<DagSnapshotEntry, _>(|key, entry| {
            if self.should_prune_transaction(&entry.tx, prune_boundary, finalized_tx_hashes) {
                keys_to_delete.push(key);
            }
        })?;

        if !keys_to_delete.is_empty() {
            nodes_pruned = storage.batch_delete_dag(&keys_to_delete)? as u64;
        }

        let checkpoint_heights_to_delete: Vec<u64> = (0..prune_boundary).collect();
        if !checkpoint_heights_to_delete.is_empty() {
            checkpoints_pruned = storage.batch_delete_checkpoints(&checkpoint_heights_to_delete)? as u64;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.stats = PruningStats {
            last_prune_time: now,
            nodes_pruned: self.stats.nodes_pruned + nodes_pruned,
            checkpoints_pruned: self.stats.checkpoints_pruned + checkpoints_pruned,
            bytes_freed: 0,
            oldest_retained_checkpoint: prune_boundary,
        };

        info!(
            "Prune complete: {} DAG nodes, {} checkpoints removed, oldest retained: {}",
            nodes_pruned, checkpoints_pruned, prune_boundary
        );

        Ok(self.stats.clone())
    }

    fn should_prune_transaction(
        &self,
        tx: &SignedTransaction,
        _prune_boundary: u64,
        finalized_tx_hashes: &HashSet<String>,
    ) -> bool {
        // CRITICAL FIX: 
        // 1. Prefer keeping unfinalized transactions to allow them to finalize
        // 2. Only prune finalized transactions that are old enough
        // 3. Safety cap: prune VERY old unfinalized transactions (10x retention) to prevent
        //    memory exhaustion from stuck transactions (e.g., permanent merkle mismatches)
        //
        // Previously this was inverted (!contains), causing unfinalized txs to be pruned
        // while finalized ones were kept forever - completely backwards!
        
        let is_finalized = finalized_tx_hashes.contains(&tx.hash);
        
        // Calculate retention periods
        // Normal retention: assume ~10s per checkpoint
        let retention_ms = self.config.retention_checkpoints * 10_000;
        // Safety cap: unfinalized transactions older than 10x retention are pruned
        // This prevents unbounded memory growth from stuck transactions
        let max_unfinalized_age_ms = retention_ms * 10;
        
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        
        let tx_age_ms = now_ms.saturating_sub(tx.tx.timestamp);
        
        if is_finalized {
            // Prune finalized transactions older than retention period
            tx_age_ms > retention_ms
        } else {
            // Safety cap: prune VERY old unfinalized transactions to prevent memory exhaustion
            // This should rarely trigger - indicates a stuck transaction that never finalized
            if tx_age_ms > max_unfinalized_age_ms {
                debug!(
                    "Pruning stuck unfinalized tx {} (age: {}ms, max: {}ms)",
                    &tx.hash[..16.min(tx.hash.len())],
                    tx_age_ms,
                    max_unfinalized_age_ms
                );
                true
            } else {
                false
            }
        }
    }

    pub fn get_stats(&self) -> &PruningStats {
        &self.stats
    }

    pub fn retention_checkpoints(&self) -> u64 {
        self.config.retention_checkpoints
    }

    pub fn estimate_prunable_nodes(
        &self,
        storage: &RedbStorage,
        current_checkpoint: u64,
        finalized_tx_hashes: &HashSet<String>,
    ) -> Result<usize> {
        let prune_boundary = self.calculate_prune_boundary(current_checkpoint);
        if prune_boundary == 0 {
            return Ok(0);
        }

        let mut count = 0;
        storage.iter_dag::<DagSnapshotEntry, _>(|_key, entry| {
            if self.should_prune_transaction(&entry.tx, prune_boundary, finalized_tx_hashes) {
                count += 1;
            }
        })?;
        Ok(count)
    }
}

pub struct InMemoryDagWindow {
    nodes: VecDeque<String>,
    max_size: usize,
    pruned_count: u64,
}

impl InMemoryDagWindow {
    pub fn new(max_size: usize) -> Self {
        Self {
            nodes: VecDeque::with_capacity(max_size),
            max_size,
            pruned_count: 0,
        }
    }

    pub fn add(&mut self, hash: String) -> Option<String> {
        self.nodes.push_back(hash);
        if self.nodes.len() > self.max_size {
            self.pruned_count += 1;
            self.nodes.pop_front()
        } else {
            None
        }
    }

    pub fn contains(&self, hash: &str) -> bool {
        self.nodes.iter().any(|h| h == hash)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn pruned_count(&self) -> u64 {
        self.pruned_count
    }

    pub fn tips(&self) -> Vec<String> {
        self.nodes.iter().rev().take(10).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prune_boundary_calculation() {
        let config = PruningConfig {
            retention_checkpoints: 500,
            ..Default::default()
        };
        let service = DagPruningService::new(config);

        assert_eq!(service.calculate_prune_boundary(50), 0);
        assert_eq!(service.calculate_prune_boundary(500), 0);
        assert_eq!(service.calculate_prune_boundary(501), 1);
        assert_eq!(service.calculate_prune_boundary(600), 100);
        assert_eq!(service.calculate_prune_boundary(1000), 500);
    }

    #[test]
    fn test_should_prune() {
        let config = PruningConfig {
            retention_checkpoints: 500,
            ..Default::default()
        };
        let service = DagPruningService::new(config);

        assert!(!service.should_prune(50));
        assert!(!service.should_prune(500));
        assert!(service.should_prune(501));
        assert!(service.should_prune(1000));
    }

    #[test]
    fn test_in_memory_dag_window() {
        let mut window = InMemoryDagWindow::new(5);

        for i in 0..5 {
            let evicted = window.add(format!("hash_{}", i));
            assert!(evicted.is_none());
        }

        assert_eq!(window.len(), 5);
        assert!(window.contains("hash_0"));
        assert!(window.contains("hash_4"));

        let evicted = window.add("hash_5".to_string());
        assert_eq!(evicted, Some("hash_0".to_string()));
        assert!(!window.contains("hash_0"));
        assert!(window.contains("hash_5"));
        assert_eq!(window.pruned_count(), 1);
    }

    #[test]
    fn test_dag_window_tips() {
        let mut window = InMemoryDagWindow::new(20);

        for i in 0..15 {
            window.add(format!("hash_{}", i));
        }

        let tips = window.tips();
        assert_eq!(tips.len(), 10);
        assert_eq!(tips[0], "hash_14");
        assert_eq!(tips[9], "hash_5");
    }
}
