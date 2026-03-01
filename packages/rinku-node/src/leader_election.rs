use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct ValidatorInfo {
    pub address: String,
    pub public_url: Option<String>,
    pub stake: u64,
    pub bls_public_key: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct LeaderElectionResult {
    pub leader_address: String,
    pub leader_url: Option<String>,
    pub is_local: bool,
    pub slot: u64,
    pub randomness: [u8; 32],
}

pub struct LeaderElection {
    local_address: String,
    local_url: Option<String>,
}

impl LeaderElection {
    pub fn new(local_address: String, local_url: Option<String>) -> Self {
        Self {
            local_address,
            local_url,
        }
    }

    pub fn elect_leader(
        &self,
        checkpoint_height: u64,
        previous_checkpoint_hash: &str,
        validators: &[ValidatorInfo],
    ) -> Option<LeaderElectionResult> {
        if validators.is_empty() {
            debug!("No validators available for leader election");
            return None;
        }

        let randomness = self.compute_randomness(checkpoint_height, previous_checkpoint_hash);
        
        let total_stake: u64 = validators.iter().map(|v| v.stake.max(1)).sum();
        if total_stake == 0 {
            warn!("Total stake is zero, cannot elect leader");
            return None;
        }

        let mut sorted_validators: Vec<_> = validators.to_vec();
        sorted_validators.sort_by(|a, b| a.address.cmp(&b.address));

        let random_value = self.randomness_to_f64(&randomness);
        let target = random_value * total_stake as f64;

        let mut cumulative = 0u64;
        for validator in &sorted_validators {
            cumulative += validator.stake.max(1);
            if cumulative as f64 >= target {
                let is_local = validator.address == self.local_address 
                    || validator.public_url.as_ref() == self.local_url.as_ref();
                
                return Some(LeaderElectionResult {
                    leader_address: validator.address.clone(),
                    leader_url: validator.public_url.clone(),
                    is_local,
                    slot: checkpoint_height,
                    randomness,
                });
            }
        }

        let first = &sorted_validators[0];
        Some(LeaderElectionResult {
            leader_address: first.address.clone(),
            leader_url: first.public_url.clone(),
            is_local: first.address == self.local_address,
            slot: checkpoint_height,
            randomness,
        })
    }

    pub fn elect_leader_from_peers(
        &self,
        checkpoint_height: u64,
        previous_checkpoint_hash: &str,
        peer_urls: &[String],
        local_url: Option<&str>,
    ) -> LeaderElectionResult {
        let randomness = self.compute_randomness(checkpoint_height, previous_checkpoint_hash);
        
        let mut all_urls: Vec<String> = peer_urls.to_vec();
        if let Some(url) = local_url {
            if !all_urls.contains(&url.to_string()) {
                all_urls.push(url.to_string());
            }
        }
        
        all_urls.sort();
        
        if all_urls.is_empty() {
            return LeaderElectionResult {
                leader_address: self.local_address.clone(),
                leader_url: self.local_url.clone(),
                is_local: true,
                slot: checkpoint_height,
                randomness,
            };
        }

        let random_index = self.randomness_to_index(&randomness, all_urls.len());
        let leader_url = &all_urls[random_index];
        
        let is_local = local_url.map(|u| u == leader_url).unwrap_or(false);
        
        LeaderElectionResult {
            leader_address: leader_url.clone(),
            leader_url: Some(leader_url.clone()),
            is_local,
            slot: checkpoint_height,
            randomness,
        }
    }

    /// Elect leader using validator addresses from the synced validator registry.
    /// This ensures ALL nodes with the same validator set will elect the same leader,
    /// regardless of their peer discovery state.
    /// 
    /// CRITICAL: Use this method instead of elect_leader_from_peers for consensus-critical
    /// leader election to prevent divergent checkpoint creation.
    pub fn elect_leader_from_validator_addresses(
        &self,
        checkpoint_height: u64,
        previous_checkpoint_hash: &str,
        validator_addresses: &[String],
        local_address: &str,
    ) -> LeaderElectionResult {
        let randomness = self.compute_randomness(checkpoint_height, previous_checkpoint_hash);
        
        // Use validator addresses (which are synced across all nodes)
        let mut all_addresses: Vec<String> = validator_addresses.to_vec();
        if !all_addresses.contains(&local_address.to_string()) {
            all_addresses.push(local_address.to_string());
        }
        
        // CRITICAL: Sort addresses for deterministic ordering across all nodes
        all_addresses.sort();
        
        if all_addresses.is_empty() {
            return LeaderElectionResult {
                leader_address: self.local_address.clone(),
                leader_url: self.local_url.clone(),
                is_local: true,
                slot: checkpoint_height,
                randomness,
            };
        }

        let random_index = self.randomness_to_index(&randomness, all_addresses.len());
        let leader_address = &all_addresses[random_index];
        
        let is_local = leader_address == local_address;
        
        debug!(
            "Leader election for checkpoint {}: {} validators, random_index={}, leader={}, is_local={}",
            checkpoint_height,
            all_addresses.len(),
            random_index,
            &leader_address[..16.min(leader_address.len())],
            is_local
        );
        
        LeaderElectionResult {
            leader_address: leader_address.clone(),
            leader_url: None, // We don't have URLs in validator registry
            is_local,
            slot: checkpoint_height,
            randomness,
        }
    }

    fn compute_randomness(&self, checkpoint_height: u64, previous_hash: &str) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"RINKU_LEADER_ELECTION_V1");
        hasher.update(checkpoint_height.to_le_bytes());
        hasher.update(previous_hash.as_bytes());
        
        let result = hasher.finalize();
        let mut randomness = [0u8; 32];
        randomness.copy_from_slice(&result);
        randomness
    }

    fn randomness_to_f64(&self, randomness: &[u8; 32]) -> f64 {
        let value = u64::from_le_bytes([
            randomness[0], randomness[1], randomness[2], randomness[3],
            randomness[4], randomness[5], randomness[6], randomness[7],
        ]);
        (value as f64) / (u64::MAX as f64)
    }

    fn randomness_to_index(&self, randomness: &[u8; 32], count: usize) -> usize {
        if count == 0 {
            return 0;
        }
        let value = u64::from_le_bytes([
            randomness[0], randomness[1], randomness[2], randomness[3],
            randomness[4], randomness[5], randomness[6], randomness[7],
        ]);
        (value as usize) % count
    }
}

#[derive(Debug, Clone)]
pub struct LeaderElectionConfig {
    pub leader_timeout_ms: u64,
    pub fallback_enabled: bool,
    pub min_validators_for_election: usize,
}

impl Default for LeaderElectionConfig {
    fn default() -> Self {
        Self {
            leader_timeout_ms: 45_000,
            fallback_enabled: true,
            min_validators_for_election: 1,
        }
    }
}

pub struct LeaderElectionService {
    election: LeaderElection,
    config: LeaderElectionConfig,
    last_checkpoint_time: std::sync::atomic::AtomicU64,
    missed_slots: std::sync::atomic::AtomicU32,
}

impl LeaderElectionService {
    pub fn new(local_address: String, local_url: Option<String>, config: LeaderElectionConfig) -> Self {
        Self {
            election: LeaderElection::new(local_address, local_url),
            config,
            last_checkpoint_time: std::sync::atomic::AtomicU64::new(0),
            missed_slots: std::sync::atomic::AtomicU32::new(0),
        }
    }

    pub fn should_create_checkpoint(
        &self,
        checkpoint_height: u64,
        previous_checkpoint_hash: &str,
        peer_urls: &[String],
        local_url: Option<&str>,
    ) -> (bool, LeaderElectionResult) {
        let result = self.election.elect_leader_from_peers(
            checkpoint_height,
            previous_checkpoint_hash,
            peer_urls,
            local_url,
        );

        let should_create = if result.is_local {
            info!(
                "LEADER ELECTION: This node elected as leader for checkpoint {} (peers: {})",
                checkpoint_height,
                peer_urls.len() + 1
            );
            true
        } else {
            debug!(
                "LEADER ELECTION: Node {} elected as leader for checkpoint {} (we are not leader)",
                result.leader_url.as_deref().unwrap_or(&result.leader_address),
                checkpoint_height
            );
            false
        };

        (should_create, result)
    }

    /// Determine if this node should create a checkpoint using validator addresses.
    /// This method uses the synced validator registry to ensure ALL nodes elect the same leader.
    /// 
    /// CRITICAL: Use this method instead of should_create_checkpoint for consensus-critical
    /// checkpoint creation to prevent divergent checkpoint creation.
    pub fn should_create_checkpoint_from_validators(
        &self,
        checkpoint_height: u64,
        previous_checkpoint_hash: &str,
        validator_addresses: &[String],
        local_address: &str,
    ) -> (bool, LeaderElectionResult) {
        let result = self.election.elect_leader_from_validator_addresses(
            checkpoint_height,
            previous_checkpoint_hash,
            validator_addresses,
            local_address,
        );

        let should_create = if result.is_local {
            info!(
                "LEADER ELECTION: This node elected as leader for checkpoint {} (validators: {})",
                checkpoint_height,
                validator_addresses.len() + 1
            );
            true
        } else {
            info!(
                "LEADER ELECTION: Validator {} elected as leader for checkpoint {} (we are not leader)",
                &result.leader_address[..16.min(result.leader_address.len())],
                checkpoint_height
            );
            false
        };

        (should_create, result)
    }

    pub fn should_fallback(
        &self,
        checkpoint_height: u64,
        previous_checkpoint_hash: &str,
        peer_urls: &[String],
        local_url: Option<&str>,
        leader_last_seen_ms: u64,
    ) -> bool {
        if !self.config.fallback_enabled {
            return false;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        if now - leader_last_seen_ms < self.config.leader_timeout_ms {
            return false;
        }

        let result = self.election.elect_leader_from_peers(
            checkpoint_height,
            previous_checkpoint_hash,
            peer_urls,
            local_url,
        );

        if result.is_local {
            return false;
        }

        let fallback_result = self.election.elect_leader_from_peers(
            checkpoint_height + 1000000,
            previous_checkpoint_hash,
            peer_urls,
            local_url,
        );

        if fallback_result.is_local {
            warn!(
                "LEADER FALLBACK: Original leader {} timed out after {}ms, this node taking over for checkpoint {}",
                result.leader_url.as_deref().unwrap_or(&result.leader_address),
                now - leader_last_seen_ms,
                checkpoint_height
            );
            self.missed_slots.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return true;
        }

        false
    }

    pub fn record_checkpoint_created(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        self.last_checkpoint_time.store(now, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn get_stats(&self) -> LeaderElectionStats {
        LeaderElectionStats {
            missed_slots: self.missed_slots.load(std::sync::atomic::Ordering::Relaxed),
            last_checkpoint_time: self.last_checkpoint_time.load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LeaderElectionStats {
    pub missed_slots: u32,
    pub last_checkpoint_time: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_leader_election() {
        let election = LeaderElection::new("node1".to_string(), Some("http://node1".to_string()));
        
        let peers = vec![
            "http://node1".to_string(),
            "http://node2".to_string(),
            "http://node3".to_string(),
        ];

        let result1 = election.elect_leader_from_peers(100, "abc123", &peers, Some("http://node1"));
        let result2 = election.elect_leader_from_peers(100, "abc123", &peers, Some("http://node1"));
        
        assert_eq!(result1.leader_url, result2.leader_url);
        assert_eq!(result1.is_local, result2.is_local);
    }

    #[test]
    fn test_different_heights_different_leaders() {
        let election = LeaderElection::new("node1".to_string(), Some("http://node1".to_string()));
        
        let peers = vec![
            "http://node1".to_string(),
            "http://node2".to_string(),
            "http://node3".to_string(),
        ];

        let mut leaders = std::collections::HashSet::new();
        for height in 0..100 {
            let result = election.elect_leader_from_peers(height, "abc123", &peers, Some("http://node1"));
            leaders.insert(result.leader_url.unwrap());
        }
        
        assert!(leaders.len() > 1, "Different heights should elect different leaders");
    }

    #[test]
    fn test_stake_weighted_election() {
        let election = LeaderElection::new("high_stake".to_string(), None);
        
        let validators = vec![
            ValidatorInfo {
                address: "high_stake".to_string(),
                public_url: None,
                stake: 100_000_000_000,
                bls_public_key: None,
            },
            ValidatorInfo {
                address: "low_stake".to_string(),
                public_url: None,
                stake: 100_000_000,
                bls_public_key: None,
            },
        ];

        let mut high_stake_wins = 0;
        for height in 0..1000 {
            let result = election.elect_leader(height, "test", &validators);
            if let Some(r) = result {
                if r.leader_address == "high_stake" {
                    high_stake_wins += 1;
                }
            }
        }
        
        assert!(high_stake_wins > 900, "High stake validator should win most elections");
    }
}
