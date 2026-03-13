use super::*;

impl NodeState {
    pub async fn get_tips(&self) -> Vec<String> {
        let state = self.inner.read().await;
        state.dag.tips()
    }

    /// Get a weighted random sample of tips for new transactions (Sparse DAG Sampling)
    /// Returns at most MAX_SAMPLED_TIPS (16) tips, preferring higher-weight tips
    /// This prevents tip explosion while maintaining DAG connectivity
    pub async fn get_sampled_tips(&self) -> Vec<String> {
        let state = self.inner.read().await;
        state.dag.get_sampled_tips()
    }

    /// Get tip count without cloning the entire tips vector (more efficient for backpressure checks)
    pub async fn get_tip_count(&self) -> usize {
        let state = self.inner.read().await;
        state.dag.tip_count()
    }

    pub async fn get_dag_stats(&self) -> (usize, usize, usize) {
        let state = self.inner.read().await;
        (
            state.dag.node_count(),
            state.dag.tip_count(),
            state.accounts.len(),
        )
    }

    pub async fn get_finalized_stats(&self) -> (usize, usize) {
        let state = self.inner.read().await;
        let total = state.dag.node_count();
        let unfinalized = state.dag.unfinalized_count();
        let finalized = total.saturating_sub(unfinalized);
        (finalized, unfinalized)
    }

    /// Returns (avg_finality_ms, median_finality_ms, p95_finality_ms, last_checkpoint_age_ms, checkpoints_per_minute)
    pub async fn get_finality_timing(&self) -> (f64, f64, f64, u64, f64) {
        let state = self.inner.read().await;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let last_checkpoint_age = now_ms.saturating_sub(state.last_checkpoint_time_ms);

        // Calculate checkpoints per minute based on genesis time
        let elapsed_minutes = (now_ms / 1000).saturating_sub(state.genesis_time) as f64 / 60.0;
        let checkpoints_per_minute = if elapsed_minutes > 0.0 {
            state.checkpoints.len() as f64 / elapsed_minutes
        } else {
            0.0
        };

        if state.finality_times_ms.is_empty() {
            return (0.0, 0.0, 0.0, last_checkpoint_age, checkpoints_per_minute);
        }

        // Use rolling window for ALL calculations (avg, median, p95)
        // This gives recent performance, not historical data polluted by stalls
        let mut times: Vec<u64> = state.finality_times_ms.iter().copied().collect();
        times.sort();

        // Calculate average from rolling window (not all-time cumulative)
        let sum: u64 = times.iter().sum();
        let avg = sum as f64 / times.len() as f64;

        let median = times[times.len() / 2] as f64;
        let p95_idx = (times.len() as f64 * 0.95) as usize;
        let p95 = times
            .get(p95_idx)
            .copied()
            .unwrap_or(times[times.len() - 1]) as f64;

        (
            avg,
            median,
            p95,
            last_checkpoint_age,
            checkpoints_per_minute,
        )
    }

    pub async fn get_all_dag_nodes(&self) -> Vec<DagNodeInfo> {
        let state = self.inner.read().await;
        state
            .dag
            .get_all_nodes()
            .into_iter()
            .map(|n| DagNodeInfo {
                hash: n.hash.clone(),
                from: n.tx.tx.from.clone(),
                to: n.tx.tx.to.clone(),
                amount: n.tx.tx.amount,
                fee: n.tx.tx.gas_price.unwrap_or(100_000),
                nonce: n.tx.tx.nonce,
                ts: n.tx.tx.timestamp,
                parents: n.parents.clone(),
                finalized: n.finalized,
                weight: n.weight,
                kind: n.tx.tx.kind,
                sig: n.tx.signature.clone(),
            })
            .collect()
    }

    /// Get paginated DAG nodes - sorted by timestamp desc, with limit
    /// Much more efficient than fetching all nodes for large DAGs
    pub async fn get_dag_nodes_paginated(&self, page: usize, limit: usize) -> (Vec<DagNodeInfo>, usize, bool) {
        let state = self.inner.read().await;
        let all_nodes = state.dag.get_all_nodes();
        let total = all_nodes.len();
        
        // Sort by timestamp descending and paginate
        let mut sorted: Vec<_> = all_nodes.into_iter().collect();
        sorted.sort_by(|a, b| b.tx.tx.timestamp.cmp(&a.tx.tx.timestamp));
        
        let start = page * limit;
        let has_more = start + limit < total;
        
        let nodes: Vec<DagNodeInfo> = sorted
            .into_iter()
            .skip(start)
            .take(limit)
            .map(|n| DagNodeInfo {
                hash: n.hash.clone(),
                from: n.tx.tx.from.clone(),
                to: n.tx.tx.to.clone(),
                amount: n.tx.tx.amount,
                fee: n.tx.tx.gas_price.unwrap_or(100_000),
                nonce: n.tx.tx.nonce,
                ts: n.tx.tx.timestamp,
                parents: n.parents.clone(),
                finalized: n.finalized,
                weight: n.weight,
                kind: n.tx.tx.kind,
                sig: n.tx.signature.clone(),
            })
            .collect();
        
        (nodes, total, has_more)
    }

    /// Combined dashboard stats - single lock acquisition for all Explorer stats
    pub async fn get_dashboard_stats(&self) -> DashboardStats {
        let state = self.inner.read().await;
        
        // Use O(1) methods instead of O(n) get_all_nodes() iteration
        // This prevents lock starvation under high transaction load
        let dag_nodes = state.dag.node_count();
        let unfinalized_count = state.dag.unfinalized_count();
        let finalized_count = dag_nodes.saturating_sub(unfinalized_count);
        
        let latest_checkpoint_id = state.checkpoints.last().map(|cp| cp.hash.clone());
        
        DashboardStats {
            dag_nodes,
            tip_count: state.dag.tip_count(),
            account_count: state.accounts.len(),
            // CRITICAL: Use actual checkpoint height, NOT len() which breaks after pruning
            checkpoint_height: state.checkpoints.last().map(|cp| cp.height).unwrap_or(0),
            finalized_count,
            unfinalized_count,
            total_transactions: std::cmp::max(state.total_transactions, dag_nodes as u64),
            tips: state.dag.tips(),
            gas_price: state.current_gas_price,
            total_burned: state.total_burned,
            avg_gas: state.current_gas_price,
            latest_checkpoint_id,
        }
    }
}
