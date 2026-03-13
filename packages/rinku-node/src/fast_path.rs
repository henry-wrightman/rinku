use rinku_core::types::{FastPathAck, FastPathFinality, FastPathStatus, SignedTransaction};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

const FAST_PATH_QUORUM_THRESHOLD: f64 = 0.667;
const FAST_PATH_TIMEOUT_MS: u64 = 5000;

#[derive(Debug, Clone)]
pub struct FastPathConfig {
    pub enabled: bool,
    pub quorum_threshold: f64,
    pub timeout_ms: u64,
}

impl Default for FastPathConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            quorum_threshold: FAST_PATH_QUORUM_THRESHOLD,
            timeout_ms: FAST_PATH_TIMEOUT_MS,
        }
    }
}

pub struct FastPathServiceInner {
    pending_txs: HashMap<String, FastPathFinality>,
    confirmed_txs: HashMap<String, FastPathFinality>,
    total_validator_stake: u64,
    config: FastPathConfig,
}

pub struct FastPathService {
    inner: Arc<RwLock<FastPathServiceInner>>,
}

impl Clone for FastPathService {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl FastPathService {
    pub fn new(config: FastPathConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(FastPathServiceInner {
                pending_txs: HashMap::new(),
                confirmed_txs: HashMap::new(),
                total_validator_stake: 0,
                config,
            })),
        }
    }

    pub async fn update_total_stake(&self, total_stake: u64) {
        let mut inner = self.inner.write().await;
        inner.total_validator_stake = total_stake;
    }

    pub async fn is_enabled(&self) -> bool {
        let inner = self.inner.read().await;
        inner.config.enabled
    }

    pub async fn register_fast_path_tx(&self, tx: &SignedTransaction) -> Option<FastPathFinality> {
        if !tx.is_fast_path_eligible() {
            return None;
        }

        let mut inner = self.inner.write().await;
        
        if inner.pending_txs.contains_key(&tx.hash) || inner.confirmed_txs.contains_key(&tx.hash) {
            return inner.pending_txs.get(&tx.hash).cloned()
                .or_else(|| inner.confirmed_txs.get(&tx.hash).cloned());
        }

        let quorum_required = (inner.total_validator_stake as f64 * inner.config.quorum_threshold) as u64;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        
        let finality = FastPathFinality {
            tx_hash: tx.hash.clone(),
            status: FastPathStatus::Pending,
            acks: Vec::new(),
            total_stake_acked: 0,
            quorum_stake_required: quorum_required,
            registered_at_ms: now_ms,
            confirmed_at_ms: None,
            checkpoint_height: None,
            tx_created_at_ms: Some(tx.tx.timestamp),
        };

        inner.pending_txs.insert(tx.hash.clone(), finality.clone());
        
        info!(
            "Registered fast-path tx {} (quorum required: {})",
            &tx.hash[..16.min(tx.hash.len())],
            quorum_required
        );

        Some(finality)
    }

    pub async fn add_ack(&self, ack: FastPathAck) -> Option<FastPathFinality> {
        let mut inner = self.inner.write().await;
        
        if let Some(finality) = inner.pending_txs.get_mut(&ack.tx_hash) {
            if finality.acks.iter().any(|a| a.validator_address == ack.validator_address) {
                debug!("Duplicate ACK from {} for {}", 
                    &ack.validator_address[..16.min(ack.validator_address.len())],
                    &ack.tx_hash[..16.min(ack.tx_hash.len())]);
                return Some(finality.clone());
            }

            finality.total_stake_acked += ack.validator_stake;
            finality.acks.push(ack.clone());

            if finality.total_stake_acked >= finality.quorum_stake_required 
                && finality.status == FastPathStatus::Pending {
                finality.status = FastPathStatus::Confirmed;
                finality.confirmed_at_ms = Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64
                );

                info!(
                    "Fast-path tx {} CONFIRMED with {}/{} stake ({} acks)",
                    &ack.tx_hash[..16.min(ack.tx_hash.len())],
                    finality.total_stake_acked,
                    finality.quorum_stake_required,
                    finality.acks.len()
                );

                let finality_clone = finality.clone();
                let tx_hash = ack.tx_hash.clone();
                
                drop(inner);
                let mut inner = self.inner.write().await;
                if let Some(f) = inner.pending_txs.remove(&tx_hash) {
                    inner.confirmed_txs.insert(tx_hash, f);
                }
                
                return Some(finality_clone);
            }

            return Some(finality.clone());
        }

        if let Some(finality) = inner.confirmed_txs.get(&ack.tx_hash) {
            return Some(finality.clone());
        }

        None
    }

    pub async fn mark_finalized(&self, tx_hash: &str, checkpoint_height: u64) {
        let mut inner = self.inner.write().await;
        
        if let Some(finality) = inner.confirmed_txs.get_mut(tx_hash) {
            finality.status = FastPathStatus::Finalized;
            finality.checkpoint_height = Some(checkpoint_height);
            
            debug!(
                "Fast-path tx {} finalized at checkpoint {}",
                &tx_hash[..16.min(tx_hash.len())],
                checkpoint_height
            );
        }

        inner.pending_txs.remove(tx_hash);
    }

    pub async fn get_status(&self, tx_hash: &str) -> Option<FastPathFinality> {
        let inner = self.inner.read().await;
        inner.pending_txs.get(tx_hash).cloned()
            .or_else(|| inner.confirmed_txs.get(tx_hash).cloned())
    }

    pub async fn is_confirmed(&self, tx_hash: &str) -> bool {
        let inner = self.inner.read().await;
        inner.confirmed_txs.contains_key(tx_hash)
    }

    pub async fn cleanup_old_confirmed(&self, max_age_ms: u64) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut inner = self.inner.write().await;
        
        let to_remove: Vec<String> = inner.confirmed_txs
            .iter()
            .filter(|(_, f)| {
                if let Some(confirmed_at) = f.confirmed_at_ms {
                    now_ms.saturating_sub(confirmed_at) > max_age_ms
                } else {
                    false
                }
            })
            .map(|(k, _)| k.clone())
            .collect();

        for tx_hash in to_remove {
            inner.confirmed_txs.remove(&tx_hash);
        }
    }

    pub async fn cleanup_timed_out_pending(&self) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut inner = self.inner.write().await;
        let timeout_ms = inner.config.timeout_ms;
        
        let to_remove: Vec<String> = inner.pending_txs
            .iter()
            .filter(|(_, f)| {
                if let Some(first_ack) = f.acks.first() {
                    now_ms.saturating_sub(first_ack.timestamp_ms) > timeout_ms
                } else {
                    false
                }
            })
            .map(|(k, _)| k.clone())
            .collect();

        for tx_hash in &to_remove {
            warn!(
                "Fast-path tx {} timed out without quorum",
                &tx_hash[..16.min(tx_hash.len())]
            );
        }

        for tx_hash in to_remove {
            inner.pending_txs.remove(&tx_hash);
        }
    }

    pub async fn get_pending_count(&self) -> usize {
        let inner = self.inner.read().await;
        inner.pending_txs.len()
    }

    pub async fn get_confirmed_count(&self) -> usize {
        let inner = self.inner.read().await;
        inner.confirmed_txs.len()
    }

    pub async fn get_stats(&self) -> FastPathStats {
        let inner = self.inner.read().await;
        
        // Calculate average confirmation time from confirmed transactions
        let mut total_ms: u64 = 0;
        let mut count: usize = 0;
        for finality in inner.confirmed_txs.values() {
            if let Some(time_ms) = finality.finality_time_ms() {
                total_ms += time_ms;
                count += 1;
            }
        }
        let avg_confirmation_ms = if count > 0 {
            Some(total_ms / count as u64)
        } else {
            None
        };
        
        FastPathStats {
            enabled: inner.config.enabled,
            pending_count: inner.pending_txs.len(),
            confirmed_count: inner.confirmed_txs.len(),
            total_validator_stake: inner.total_validator_stake,
            quorum_threshold: inner.config.quorum_threshold,
            avg_confirmation_ms,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FastPathStats {
    pub enabled: bool,
    pub pending_count: usize,
    pub confirmed_count: usize,
    pub total_validator_stake: u64,
    pub quorum_threshold: f64,
    pub avg_confirmation_ms: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FastPathBroadcast {
    pub tx: SignedTransaction,
    pub sender_validator: String,
    pub sender_stake: u64,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FastPathAckMessage {
    pub tx_hash: String,
    pub validator_address: String,
    pub validator_stake: u64,
    pub bls_signature: Option<String>,
    pub timestamp_ms: u64,
}

impl From<FastPathAckMessage> for FastPathAck {
    fn from(msg: FastPathAckMessage) -> Self {
        FastPathAck {
            tx_hash: msg.tx_hash,
            validator_address: msg.validator_address,
            validator_stake: msg.validator_stake,
            bls_signature: msg.bls_signature,
            timestamp_ms: msg.timestamp_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rinku_core::types::{Transaction, TransactionKind};

    fn create_test_tx(amount: u64, memo: Option<String>) -> SignedTransaction {
        SignedTransaction {
            tx: Transaction {
                from: "test_sender".to_string(),
                to: "test_receiver".to_string(),
                amount,
                nonce: 1,
                timestamp: 1000,
                parents: vec![],
                kind: if amount == 0 { Some(TransactionKind::DataOnly) } else { Some(TransactionKind::Transfer) },
                gas_limit: Some(21000),
                gas_price: Some(100_000),
                data: None,
                signature: None,
                memo,
                references: None,
            },
            hash: "abc123".to_string(),
            signature: "sig".to_string(),
        }
    }

    #[tokio::test]
    async fn test_fast_path_eligibility() {
        let data_only_tx = create_test_tx(0, Some("Hello world".to_string()));
        assert!(data_only_tx.is_fast_path_eligible());

        let transfer_tx = create_test_tx(1_000_000_000, None);
        assert!(transfer_tx.is_fast_path_eligible());
    }

    #[tokio::test]
    async fn test_fast_path_quorum() {
        let service = FastPathService::new(FastPathConfig::default());
        service.update_total_stake(100_000_000_000).await;

        let tx = create_test_tx(0, Some("Test message".to_string()));
        let finality = service.register_fast_path_tx(&tx).await.unwrap();
        
        assert_eq!(finality.status, FastPathStatus::Pending);
        assert!(finality.quorum_stake_required > 0);

        let ack1 = FastPathAck {
            tx_hash: tx.hash.clone(),
            validator_address: "validator1".to_string(),
            validator_stake: 40_000_000_000,
            bls_signature: None,
            timestamp_ms: 1000,
        };
        let result = service.add_ack(ack1).await.unwrap();
        assert_eq!(result.status, FastPathStatus::Pending);

        let ack2 = FastPathAck {
            tx_hash: tx.hash.clone(),
            validator_address: "validator2".to_string(),
            validator_stake: 40_000_000_000,
            bls_signature: None,
            timestamp_ms: 1001,
        };
        let result = service.add_ack(ack2).await.unwrap();
        assert_eq!(result.status, FastPathStatus::Confirmed);
        assert!(result.confirmed_at_ms.is_some());
    }
}
