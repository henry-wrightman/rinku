use super::*;

impl NodeState {
    pub async fn calculate_cumulative_weight(&self, hash: &str) -> f64 {
        let state = self.inner.read().await;
        state.dag.calculate_cumulative_weight(hash)
    }

    pub async fn prune_losing_branch(&self, loser_hash: &str) -> Result<usize> {
        let mut state = self.inner.write().await;

        if let Some(node) = state.dag.get_node(loser_hash) {
            if node.finalized {
                info!(
                    "Cannot prune finalized node {}",
                    &loser_hash[..16.min(loser_hash.len())]
                );
                return Ok(0);
            }
        }

        let removed_nodes = state.dag.prune_branch(loser_hash);
        let pruned_count = removed_nodes.len();

        for node in &removed_nodes {
            let tx = &node.tx.tx;

            if let Some(from_account) = state.accounts.get_mut(&tx.from) {
                from_account.balance += tx.amount;
                if let Some(gas_price) = tx.gas_price {
                    let gas_limit = tx.gas_limit.unwrap_or(21000);
                    from_account.balance += gas_price * gas_limit as f64;
                }
                if from_account.nonce > 0 {
                    from_account.nonce -= 1;
                }
            }

            if let Some(to_account) = state.accounts.get_mut(&tx.to) {
                to_account.balance -= tx.amount;
                if to_account.balance < 0.0 {
                    to_account.balance = 0.0;
                }
            }
        }

        info!(
            "Pruned {} transactions from losing branch starting at {}, reverted account balances",
            pruned_count,
            &loser_hash[..16.min(loser_hash.len())]
        );

        Ok(pruned_count)
    }

    pub async fn resolve_fork(
        &self,
        tip_a: &str,
        tip_b: &str,
    ) -> Option<(String, String, f64, f64)> {
        let state = self.inner.read().await;

        let weight_a = state.dag.calculate_cumulative_weight(tip_a);
        let weight_b = state.dag.calculate_cumulative_weight(tip_b);

        if (weight_a - weight_b).abs() < 0.001 {
            return None;
        }

        let (winner, loser) = if weight_a > weight_b {
            (tip_a.to_string(), tip_b.to_string())
        } else {
            (tip_b.to_string(), tip_a.to_string())
        };

        Some((winner, loser, weight_a, weight_b))
    }
}
