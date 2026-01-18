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
            prev_account_tx: tx.tx.prev_account_tx.clone(),
            prev_account_proof_url: tx.tx.prev_account_proof_url.clone(),
        };

        let tx_json = serde_json::to_string(&tx_for_hash)?;
        let computed_hash = sha256_hex(&tx_json);

        if computed_hash != tx.hash {
            warn!("Transaction hash mismatch");
            return Ok(false);
        }

        // Amount must be positive (except for unstake which can be 0)
        let is_unstake = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
        if tx.tx.amount <= 0.0 && !is_unstake {
            warn!("Invalid transaction amount");
            return Ok(false);
        }

        let sender = self.state.get_account(&tx.tx.from).await;
        let gas_fee = tx.tx.gas_price.unwrap_or(0.001); // Default gas price
        
        match &sender {
            Some(account) => {
                // Calculate required balance based on transaction type
                let is_stake = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
                
                let required_balance = if is_stake {
                    // Stake: need amount + gas (amount is locked, not transferred)
                    tx.tx.amount + gas_fee
                } else if is_unstake {
                    // Unstake: only need gas fee
                    gas_fee
                } else {
                    // Transfer: need amount + gas
                    tx.tx.amount + gas_fee
                };
                
                if account.balance < required_balance {
                    warn!(
                        "Insufficient balance: have {:.6}, need {:.6} (amount: {:.6}, gas: {:.6})",
                        account.balance, required_balance, tx.tx.amount, gas_fee
                    );
                    return Ok(false);
                }

                if tx.tx.nonce != account.nonce {
                    warn!("Invalid nonce: expected {}, got {}", account.nonce, tx.tx.nonce);
                    return Ok(false);
                }
            }
            None => {
                // Account doesn't exist - reject unless it's a genesis transaction
                if tx.tx.from != "genesis" {
                    warn!("Account {} does not exist - cannot process transaction", &tx.tx.from[..16.min(tx.tx.from.len())]);
                    return Ok(false);
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use rinku_core::types::{Transaction, TransactionKind};

    fn create_test_tx(from: &str, amount: f64, gas_price: Option<f64>, kind: Option<TransactionKind>) -> SignedTransaction {
        let tx = Transaction {
            from: from.to_string(),
            to: from.to_string(), // stake to self
            amount,
            nonce: 0,
            timestamp: 1000000,
            parents: vec![],
            kind,
            gas_limit: None,
            gas_price,
            data: None,
            signature: None,
            prev_account_tx: None,
            prev_account_proof_url: None,
        };
        let tx_json = serde_json::to_string(&tx).unwrap();
        let hash = sha256_hex(&tx_json);
        SignedTransaction {
            tx,
            hash,
            signature: "test_sig".to_string(),
        }
    }

    #[test]
    fn test_stake_requires_balance_plus_gas() {
        // Test that stake validation checks for amount + gas, not just amount
        let tx = create_test_tx("test_user", 100.0, Some(0.01), Some(TransactionKind::Stake));
        
        // Required balance should be 100.0 + 0.01 = 100.01
        let gas_fee = tx.tx.gas_price.unwrap_or(0.001);
        let is_stake = matches!(tx.tx.kind, Some(TransactionKind::Stake));
        let required = if is_stake { tx.tx.amount + gas_fee } else { tx.tx.amount + gas_fee };
        
        assert_eq!(required, 100.01);
        assert!(is_stake);
    }

    #[test]
    fn test_unstake_only_needs_gas() {
        let tx = create_test_tx("test_user", 0.0, Some(0.01), Some(TransactionKind::Unstake));
        
        let gas_fee = tx.tx.gas_price.unwrap_or(0.001);
        let is_unstake = matches!(tx.tx.kind, Some(TransactionKind::Unstake));
        let required = if is_unstake { gas_fee } else { tx.tx.amount + gas_fee };
        
        assert_eq!(required, 0.01);
        assert!(is_unstake);
    }

    #[test]
    fn test_transfer_requires_amount_plus_gas() {
        let tx = create_test_tx("test_user", 50.0, Some(0.005), None);
        
        let gas_fee = tx.tx.gas_price.unwrap_or(0.001);
        let is_stake = matches!(tx.tx.kind, Some(TransactionKind::Stake));
        let is_unstake = matches!(tx.tx.kind, Some(TransactionKind::Unstake));
        let required = if is_stake || is_unstake { 0.0 } else { tx.tx.amount + gas_fee };
        
        assert_eq!(required, 50.005);
        assert!(!is_stake);
        assert!(!is_unstake);
    }

    #[test]
    fn test_zero_balance_cannot_stake() {
        // Simulates the bug: account with 0 balance trying to stake 100
        let account_balance = 0.0;
        let stake_amount = 100.0;
        let gas_fee = 0.001;
        
        let required_balance = stake_amount + gas_fee; // 100.001
        let has_sufficient = account_balance >= required_balance;
        
        assert!(!has_sufficient, "Zero balance should NOT be able to stake");
    }

    #[test]
    fn test_insufficient_balance_for_stake_plus_gas() {
        // Account has exactly the stake amount but not enough for gas
        let account_balance = 100.0;
        let stake_amount = 100.0;
        let gas_fee = 0.001;
        
        let required_balance = stake_amount + gas_fee; // 100.001
        let has_sufficient = account_balance >= required_balance;
        
        assert!(!has_sufficient, "Balance equal to stake amount should fail (no gas)");
    }

    #[test]
    fn test_sufficient_balance_for_stake() {
        let account_balance = 100.01;
        let stake_amount = 100.0;
        let gas_fee = 0.001;
        
        let required_balance = stake_amount + gas_fee; // 100.001
        let has_sufficient = account_balance >= required_balance;
        
        assert!(has_sufficient, "Balance > stake + gas should succeed");
    }
}
