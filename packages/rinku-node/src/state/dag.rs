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

    /// Single-snapshot helper for `/finality/metrics`: combines what was previously
    /// three back-to-back `inner.read()` acquisitions (`get_total_transactions`,
    /// `get_finalized_stats`, `get_finality_timing`) into one. Reduces per-request
    /// lock churn and the chance of queueing behind a checkpoint-apply writer.
    ///
    /// Returns: (total_transactions, finalized_count, pending_count,
    ///           avg_finality_ms, median_finality_ms, p95_finality_ms,
    ///           last_checkpoint_age_ms, checkpoints_per_minute)
    pub async fn get_finality_summary(&self) -> (u64, usize, usize, f64, f64, f64, u64, f64) {
        let state = self.inner.read().await;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let total = state.dag.node_count();
        let unfinalized = state.dag.unfinalized_count();
        let finalized = total.saturating_sub(unfinalized);
        let total_transactions =
            std::cmp::max(state.total_transactions, state.dag.node_count() as u64);

        let last_checkpoint_age = now_ms.saturating_sub(state.last_checkpoint_time_ms);

        let checkpoints_per_minute = if state.checkpoints.len() >= 2 {
            let window_size = state.checkpoints.len().min(120);
            let start_idx = state.checkpoints.len() - window_size;
            let oldest_ts = state.checkpoints[start_idx].timestamp;
            let newest_ts = state.checkpoints[state.checkpoints.len() - 1].timestamp;
            let window_secs = newest_ts.saturating_sub(oldest_ts);
            if window_secs > 0 {
                (window_size.saturating_sub(1)) as f64 / (window_secs as f64 / 60.0)
            } else {
                0.0
            }
        } else {
            0.0
        };

        if state.finality_times_ms.is_empty() {
            return (
                total_transactions,
                finalized,
                unfinalized,
                0.0,
                0.0,
                0.0,
                last_checkpoint_age,
                checkpoints_per_minute,
            );
        }

        let mut times: Vec<u64> = state.finality_times_ms.iter().copied().collect();
        times.sort();
        let sum: u64 = times.iter().sum();
        let avg = sum as f64 / times.len() as f64;
        let median = times[times.len() / 2] as f64;
        let p95_idx = (times.len() as f64 * 0.95) as usize;
        let p95 = times
            .get(p95_idx)
            .copied()
            .unwrap_or(times[times.len() - 1]) as f64;

        (
            total_transactions,
            finalized,
            unfinalized,
            avg,
            median,
            p95,
            last_checkpoint_age,
            checkpoints_per_minute,
        )
    }

    /// Returns (avg_finality_ms, median_finality_ms, p95_finality_ms, last_checkpoint_age_ms, checkpoints_per_minute)
    pub async fn get_finality_timing(&self) -> (f64, f64, f64, u64, f64) {
        let state = self.inner.read().await;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let last_checkpoint_age = now_ms.saturating_sub(state.last_checkpoint_time_ms);

        let checkpoints_per_minute = if state.checkpoints.len() >= 2 {
            let window_size = state.checkpoints.len().min(120);
            let start_idx = state.checkpoints.len() - window_size;
            let oldest_ts = state.checkpoints[start_idx].timestamp;
            let newest_ts = state.checkpoints[state.checkpoints.len() - 1].timestamp;
            let window_secs = newest_ts.saturating_sub(oldest_ts);
            if window_secs > 0 {
                (window_size.saturating_sub(1)) as f64 / (window_secs as f64 / 60.0)
            } else {
                0.0
            }
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
                effective_amount: n.effective_amount,
            })
            .collect()
    }

    /// Get paginated DAG nodes - sorted by timestamp desc, with limit.
    ///
    /// Optimized to minimize read-lock hold time:
    /// - Collects references (not clones) of all DAG nodes (O(N) pointers).
    /// - Uses `select_nth_unstable_by` to partition in O(N) instead of full O(N log N) sort.
    /// - Only the page window is sorted and cloned into `DagNodeInfo`.
    /// This keeps the `inner.read()` hold time well under 1ms even at thousands of nodes,
    /// avoiding contention with the checkpoint write lock.
    pub async fn get_dag_nodes_paginated(&self, page: usize, limit: usize) -> (Vec<DagNodeInfo>, usize, bool) {
        let state = self.inner.read().await;
        let mut node_refs: Vec<&_> = state.dag.get_all_nodes();
        let total = node_refs.len();

        let start = page * limit;
        let has_more = start.saturating_add(limit) < total;

        if start >= total {
            return (Vec::new(), total, false);
        }

        // Total ordering: timestamp desc, then hash desc as tiebreaker.
        // Required because `select_nth_unstable_by` is unstable — a timestamp-only
        // comparator would let ties re-order across the partition boundary, causing
        // page overlaps/gaps at high TPS where many txs share a ms.
        // Comparator is inlined at both call sites because `DagNode` is not exported
        // from `rinku_core`, so it can't be named in a `let cmp = |...|` binding.

        // Partial selection: keep only the top (start + limit) by the total order, in O(N).
        let needed = start.saturating_add(limit).min(total);
        if needed > 0 && needed < total {
            node_refs.select_nth_unstable_by(needed - 1, |a, b| {
                b.tx.tx.timestamp
                    .cmp(&a.tx.tx.timestamp)
                    .then_with(|| b.hash.cmp(&a.hash))
            });
            node_refs.truncate(needed);
        }
        // Final sort over the small window only.
        node_refs.sort_by(|a, b| {
            b.tx.tx.timestamp
                .cmp(&a.tx.tx.timestamp)
                .then_with(|| b.hash.cmp(&a.hash))
        });

        let nodes: Vec<DagNodeInfo> = node_refs
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
                effective_amount: n.effective_amount,
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
