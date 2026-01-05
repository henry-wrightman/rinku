use anyhow::Result;
use rinku_core::{
    crypto::{sha256_hex, verify_signature},
    types::{SignedTransaction, Transaction},
    weight::calculate_account_weight,
};
use tracing::{debug, warn};

use crate::state::NodeState;

pub struct ConsensusService {
    state: NodeState,
}

impl ConsensusService {
    pub fn new(state: NodeState) -> Self {
        Self { state }
    }

    pub async fn validate_transaction(&self, tx: &SignedTransaction) -> Result<bool> {
        let tx_for_hash = Transaction {
            from: tx.tx.from.clone(),
            to: tx.tx.to.clone(),
            amount: tx.tx.amount,
            nonce: tx.tx.nonce,
            timestamp: tx.tx.timestamp,
            parents: tx.tx.parents.clone(),
            kind: tx.tx.kind,
            gas_limit: tx.tx.gas_limit,
            gas_price: tx.tx.gas_price,
            data: tx.tx.data.clone(),
            signature: None,
        };

        let tx_json = serde_json::to_string(&tx_for_hash)?;
        let computed_hash = sha256_hex(&tx_json);

        if computed_hash != tx.hash {
            warn!("Transaction hash mismatch");
            return Ok(false);
        }

        if tx.tx.amount <= 0.0 {
            warn!("Invalid transaction amount");
            return Ok(false);
        }

        let sender = self.state.get_account(&tx.tx.from).await;
        if let Some(account) = &sender {
            if account.balance < tx.tx.amount {
                warn!("Insufficient balance");
                return Ok(false);
            }

            if tx.tx.nonce != account.nonce {
                warn!("Invalid nonce: expected {}, got {}", account.nonce, tx.tx.nonce);
                return Ok(false);
            }
        }

        for parent in &tx.tx.parents {
            if !parent.is_empty() {
                let state = self.state.inner.read().await;
                if !state.dag.contains(parent) {
                    warn!("Parent not found: {}", parent);
                    return Ok(false);
                }
            }
        }

        debug!("Transaction {} validated successfully", &tx.hash[..16]);
        Ok(true)
    }

    pub async fn calculate_transaction_weight(&self, tx: &SignedTransaction) -> f64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if let Some(account) = self.state.get_account(&tx.tx.from).await {
            calculate_account_weight(&account, now)
        } else {
            1.0
        }
    }
}
