use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const PROTOCOL_VERSION: &str = "1.0.0";
pub const ACTIVATION_THRESHOLD: f64 = 0.75;
pub const UPGRADE_EXPIRY_MS: u64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl ProtocolVersion {
    pub fn parse(version: &str) -> Option<Self> {
        let parts: Vec<&str> = version.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        Some(ProtocolVersion {
            major: parts[0].parse().ok()?,
            minor: parts[1].parse().ok()?,
            patch: parts[2].parse().ok()?,
        })
    }

    pub fn to_string(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }

    pub fn is_compatible(&self, other: &ProtocolVersion) -> bool {
        self.major == other.major
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FeatureStatus {
    Proposed,
    Signaling,
    LockedIn,
    Active,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureFlag {
    pub id: String,
    pub name: String,
    pub description: String,
    pub activation_height: Option<u64>,
    pub activation_threshold: f64,
    pub status: FeatureStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeSignal {
    pub validator: String,
    pub version: String,
    pub features: Vec<String>,
    pub timestamp: u64,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub protocol_version: String,
    pub node_version: String,
    pub chain_id: String,
    pub network_id: String,
    pub features: Vec<FeatureFlag>,
    pub min_compatible_version: String,
    pub activation_height: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeStatus {
    Proposed,
    Signaling,
    LockedIn,
    Active,
    Rejected,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeProposal {
    pub id: String,
    pub title: String,
    pub description: String,
    pub target_version: String,
    pub features: Vec<String>,
    pub proposed_at: u64,
    pub proposed_by: String,
    pub activation_threshold: f64,
    pub activation_height: Option<u64>,
    pub signal_count: u64,
    pub signal_weight: f64,
    pub total_weight: f64,
    pub status: UpgradeStatus,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionCompatibility {
    pub compatible: bool,
    pub local_version: String,
    pub remote_version: String,
    pub reason: Option<String>,
    pub can_connect: bool,
    pub can_sync: bool,
}

pub fn get_known_features() -> HashMap<String, FeatureFlag> {
    let mut features = HashMap::new();

    features.insert(
        "bls-aggregation".to_string(),
        FeatureFlag {
            id: "bls-aggregation".to_string(),
            name: "BLS Signature Aggregation".to_string(),
            description: "Use BLS12-381 aggregated signatures for compact checkpoint proofs"
                .to_string(),
            activation_threshold: 0.75,
            activation_height: Some(0),
            status: FeatureStatus::Active,
        },
    );

    features.insert(
        "zk-privacy".to_string(),
        FeatureFlag {
            id: "zk-privacy".to_string(),
            name: "ZK Privacy Layer".to_string(),
            description: "Enable optional privacy-preserving proofs using Groth16 ZK-SNARKs"
                .to_string(),
            activation_threshold: 0.80,
            activation_height: None,
            status: FeatureStatus::Proposed,
        },
    );

    features.insert(
        "dynamic-gas".to_string(),
        FeatureFlag {
            id: "dynamic-gas".to_string(),
            name: "Dynamic Gas Pricing".to_string(),
            description: "EIP-1559 style utilization-based gas fee adjustment".to_string(),
            activation_threshold: 0.75,
            activation_height: Some(0),
            status: FeatureStatus::Active,
        },
    );

    features.insert(
        "smart-contracts".to_string(),
        FeatureFlag {
            id: "smart-contracts".to_string(),
            name: "Smart Contracts".to_string(),
            description: "URL-encoded smart contract execution".to_string(),
            activation_threshold: 0.75,
            activation_height: Some(0),
            status: FeatureStatus::Active,
        },
    );

    features.insert(
        "tip-consolidation".to_string(),
        FeatureFlag {
            id: "tip-consolidation".to_string(),
            name: "Protocol-Level Tip Consolidation".to_string(),
            description: "Automatic DAG tip reduction via validator consolidation transactions"
                .to_string(),
            activation_threshold: 0.75,
            activation_height: Some(0),
            status: FeatureStatus::Active,
        },
    );

    features
}

pub fn check_version_compatibility(local: &str, remote: &str) -> VersionCompatibility {
    let local_version = ProtocolVersion::parse(local);
    let remote_version = ProtocolVersion::parse(remote);

    match (local_version, remote_version) {
        (Some(lv), Some(rv)) => {
            let compatible = lv.is_compatible(&rv);
            let can_connect = compatible;
            let can_sync = compatible && lv.minor >= rv.minor.saturating_sub(1);

            let reason = if !compatible {
                Some(format!(
                    "Major version mismatch: {} vs {}",
                    lv.major, rv.major
                ))
            } else if !can_sync {
                Some(format!(
                    "Minor version too far behind: {} vs {}",
                    lv.minor, rv.minor
                ))
            } else {
                None
            };

            VersionCompatibility {
                compatible,
                local_version: local.to_string(),
                remote_version: remote.to_string(),
                reason,
                can_connect,
                can_sync,
            }
        }
        _ => VersionCompatibility {
            compatible: false,
            local_version: local.to_string(),
            remote_version: remote.to_string(),
            reason: Some("Failed to parse version".to_string()),
            can_connect: false,
            can_sync: false,
        },
    }
}

pub struct VersioningService {
    pub version_info: VersionInfo,
    pub proposals: Vec<UpgradeProposal>,
    pub signals: Vec<UpgradeSignal>,
}

impl VersioningService {
    pub fn new(chain_id: &str, network_id: &str) -> Self {
        let features: Vec<FeatureFlag> = get_known_features()
            .into_values()
            .filter(|f| f.status == FeatureStatus::Active)
            .collect();

        VersioningService {
            version_info: VersionInfo {
                protocol_version: PROTOCOL_VERSION.to_string(),
                node_version: env!("CARGO_PKG_VERSION").to_string(),
                chain_id: chain_id.to_string(),
                network_id: network_id.to_string(),
                features,
                min_compatible_version: "1.0.0".to_string(),
                activation_height: 0,
            },
            proposals: Vec::new(),
            signals: Vec::new(),
        }
    }

    pub fn get_version_info(&self) -> &VersionInfo {
        &self.version_info
    }

    pub fn get_active_features(&self) -> Vec<&FeatureFlag> {
        self.version_info
            .features
            .iter()
            .filter(|f| f.status == FeatureStatus::Active)
            .collect()
    }

    pub fn is_feature_active(&self, feature_id: &str) -> bool {
        self.version_info
            .features
            .iter()
            .any(|f| f.id == feature_id && f.status == FeatureStatus::Active)
    }

    pub fn add_proposal(&mut self, proposal: UpgradeProposal) {
        self.proposals.push(proposal);
    }

    pub fn record_signal(&mut self, signal: UpgradeSignal) {
        self.signals.push(signal);
    }

    pub fn get_proposals(&self) -> &[UpgradeProposal] {
        &self.proposals
    }

    pub fn check_compatibility(&self, remote_version: &str) -> VersionCompatibility {
        check_version_compatibility(&self.version_info.protocol_version, remote_version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        let v = ProtocolVersion::parse("1.2.3").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
    }

    #[test]
    fn test_version_compatibility() {
        let v1 = ProtocolVersion::parse("1.0.0").unwrap();
        let v2 = ProtocolVersion::parse("1.1.0").unwrap();
        let v3 = ProtocolVersion::parse("2.0.0").unwrap();

        assert!(v1.is_compatible(&v2));
        assert!(!v1.is_compatible(&v3));
    }

    #[test]
    fn test_check_version_compatibility() {
        let result = check_version_compatibility("1.0.0", "1.1.0");
        assert!(result.compatible);
        assert!(result.can_connect);

        let result2 = check_version_compatibility("1.0.0", "2.0.0");
        assert!(!result2.compatible);
    }

    #[test]
    fn test_known_features() {
        let features = get_known_features();
        assert!(features.contains_key("bls-aggregation"));
        assert!(features.contains_key("dynamic-gas"));
        assert!(features.contains_key("smart-contracts"));
    }

    #[test]
    fn test_versioning_service() {
        let service = VersioningService::new("rinku-testnet", "testnet");

        assert_eq!(service.version_info.chain_id, "rinku-testnet");
        assert!(service.is_feature_active("bls-aggregation"));
        assert!(service.is_feature_active("dynamic-gas"));
        assert!(!service.is_feature_active("zk-privacy"));
    }
}
