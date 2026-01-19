use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

pub const UNBONDING_PERIOD_MS: u64 = 14 * 24 * 60 * 60 * 1000;
pub const SLASH_DOUBLE_SIGN_PERCENT: f64 = 0.15;
pub const DOUBLE_SIGN_EVIDENCE_EXPIRY_MS: u64 = 24 * 60 * 60 * 1000;
pub const SLASH_INVALID_CHECKPOINT_PERCENT: f64 = 0.25;
pub const SLASH_INVALID_PROOF_PERCENT: f64 = 0.20;
pub const SLASH_INVALID_WITNESS_PERCENT: f64 = 0.15;
pub const SLASH_RECEIPT_TAMPERING_PERCENT: f64 = 0.25;
pub const SLASH_LIVENESS_PERCENT: f64 = 0.05;
pub const SLASH_LIVENESS_REPEAT_PERCENT: f64 = 0.10;
pub const LIVENESS_MISS_THRESHOLD: u32 = 3;
pub const LIVENESS_REPEAT_WINDOW_MS: u64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlashReason {
    DoubleSign,
    InvalidCheckpoint,
    InvalidProof,
    InvalidWitness,
    ReceiptTampering,
    LivenessFailure,
    LivenessRepeat,
}

impl SlashReason {
    pub fn percent(&self) -> f64 {
        match self {
            SlashReason::DoubleSign => SLASH_DOUBLE_SIGN_PERCENT,
            SlashReason::InvalidCheckpoint => SLASH_INVALID_CHECKPOINT_PERCENT,
            SlashReason::InvalidProof => SLASH_INVALID_PROOF_PERCENT,
            SlashReason::InvalidWitness => SLASH_INVALID_WITNESS_PERCENT,
            SlashReason::ReceiptTampering => SLASH_RECEIPT_TAMPERING_PERCENT,
            SlashReason::LivenessFailure => SLASH_LIVENESS_PERCENT,
            SlashReason::LivenessRepeat => SLASH_LIVENESS_REPEAT_PERCENT,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashEvent {
    pub id: String,
    pub validator: String,
    pub reason: SlashReason,
    pub amount: f64,
    pub percent_slashed: f64,
    pub checkpoint_height: u64,
    pub timestamp: u64,
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnbondingEntry {
    pub validator: String,
    pub amount: f64,
    pub started_at: u64,
    pub available_at: u64,
    pub slashable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LivenessRecord {
    count: u32,
    last_failure: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoubleSignEvidence {
    pub validator: String,
    pub checkpoint_height: u64,
    pub hash1: String,
    pub hash2: String,
    pub signature1: String,
    pub signature2: Option<String>,
    pub timestamp: u64,
    pub processed: bool,
}

pub struct SlashingService {
    slash_events: Vec<SlashEvent>,
    unbonding_queue: Vec<UnbondingEntry>,
    liveness_failures: HashMap<String, LivenessRecord>,
    double_sign_evidence: Vec<DoubleSignEvidence>,
    total_slashed: f64,
    next_slash_id: u64,
}

impl SlashingService {
    pub fn new() -> Self {
        Self {
            double_sign_evidence: Vec::new(),
            slash_events: Vec::new(),
            unbonding_queue: Vec::new(),
            liveness_failures: HashMap::new(),
            total_slashed: 0.0,
            next_slash_id: 1,
        }
    }

    fn current_time_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    pub fn slash(
        &mut self,
        validator: &str,
        stake_amount: f64,
        reason: SlashReason,
        checkpoint_height: u64,
        details: Option<String>,
    ) -> Option<SlashEvent> {
        if stake_amount <= 0.0 {
            return None;
        }

        let percent = reason.percent();
        let amount = stake_amount * percent;
        let now = Self::current_time_ms();

        let event = SlashEvent {
            id: format!("slash_{}", self.next_slash_id),
            validator: validator.to_string(),
            reason,
            amount,
            percent_slashed: percent * 100.0,
            checkpoint_height,
            timestamp: now,
            details,
        };

        self.next_slash_id += 1;
        self.total_slashed += amount;
        self.slash_events.push(event.clone());

        if self.slash_events.len() > 1000 {
            self.slash_events = self.slash_events.split_off(self.slash_events.len() - 500);
        }

        warn!(
            "Slashed {} for {:?}: {} RKU ({}%)",
            validator,
            reason,
            amount,
            percent * 100.0
        );

        Some(event)
    }

    pub fn record_liveness_failure(
        &mut self,
        validator: &str,
        checkpoint_height: u64,
        stake_amount: f64,
    ) -> Option<SlashEvent> {
        let now = Self::current_time_ms();

        let record = self
            .liveness_failures
            .entry(validator.to_string())
            .or_insert(LivenessRecord {
                count: 0,
                last_failure: now,
            });

        let within_repeat_window = (now - record.last_failure) < LIVENESS_REPEAT_WINDOW_MS;
        record.count += 1;
        record.last_failure = now;

        if record.count >= LIVENESS_MISS_THRESHOLD {
            let is_repeat = within_repeat_window && record.count > LIVENESS_MISS_THRESHOLD;
            let reason = if is_repeat {
                SlashReason::LivenessRepeat
            } else {
                SlashReason::LivenessFailure
            };

            return self.slash(
                validator,
                stake_amount,
                reason,
                checkpoint_height,
                Some(format!(
                    "Missed {} consecutive checkpoints",
                    LIVENESS_MISS_THRESHOLD
                )),
            );
        }

        None
    }

    pub fn reset_liveness_counter(&mut self, validator: &str) {
        self.liveness_failures.remove(validator);
    }

    pub fn submit_double_sign_evidence(
        &mut self,
        validator: String,
        checkpoint_height: u64,
        hash1: String,
        hash2: String,
        signature1: String,
        signature2: Option<String>,
    ) -> bool {
        if hash1 == hash2 {
            return false;
        }

        let already_exists = self.double_sign_evidence.iter().any(|e| {
            e.validator == validator
                && e.checkpoint_height == checkpoint_height
                && ((e.hash1 == hash1 && e.hash2 == hash2)
                    || (e.hash1 == hash2 && e.hash2 == hash1))
        });

        if already_exists {
            return false;
        }

        let evidence = DoubleSignEvidence {
            validator: validator.clone(),
            checkpoint_height,
            hash1,
            hash2,
            signature1,
            signature2,
            timestamp: Self::current_time_ms(),
            processed: false,
        };

        info!(
            "Double-sign evidence submitted for validator {} at height {}",
            validator, checkpoint_height
        );

        self.double_sign_evidence.push(evidence);
        true
    }

    pub fn process_double_sign_evidence(&mut self, stake_amount: f64) -> Vec<SlashEvent> {
        let mut events = Vec::new();
        let now = Self::current_time_ms();

        let mut to_slash: Vec<(String, u64, String, String)> = Vec::new();

        for evidence in &mut self.double_sign_evidence {
            if evidence.processed {
                continue;
            }

            if now - evidence.timestamp > DOUBLE_SIGN_EVIDENCE_EXPIRY_MS {
                evidence.processed = true;
                continue;
            }

            to_slash.push((
                evidence.validator.clone(),
                evidence.checkpoint_height,
                evidence.hash1.clone(),
                evidence.hash2.clone(),
            ));
            evidence.processed = true;
        }

        for (validator, height, hash1, hash2) in to_slash {
            if let Some(event) = self.slash(
                &validator,
                stake_amount,
                SlashReason::DoubleSign,
                height,
                Some(format!(
                    "Double-signed checkpoint: {} vs {}",
                    &hash1[..16.min(hash1.len())],
                    &hash2[..16.min(hash2.len())]
                )),
            ) {
                events.push(event);
            }
        }

        self.double_sign_evidence.retain(|e| {
            !e.processed || (now - e.timestamp < DOUBLE_SIGN_EVIDENCE_EXPIRY_MS)
        });

        events
    }

    pub fn get_pending_evidence(&self) -> Vec<&DoubleSignEvidence> {
        self.double_sign_evidence
            .iter()
            .filter(|e| !e.processed)
            .collect()
    }

    pub fn start_unbonding(&mut self, validator: &str, amount: f64) -> UnbondingEntry {
        let now = Self::current_time_ms();

        let entry = UnbondingEntry {
            validator: validator.to_string(),
            amount,
            started_at: now,
            available_at: now + UNBONDING_PERIOD_MS,
            slashable: true,
        };

        self.unbonding_queue.push(entry.clone());
        entry
    }

    pub fn process_unbonding_queue(&mut self) -> Vec<UnbondingEntry> {
        let now = Self::current_time_ms();

        let (ready, pending): (Vec<_>, Vec<_>) = self
            .unbonding_queue
            .drain(..)
            .partition(|e| e.available_at <= now && e.slashable);

        self.unbonding_queue = pending;
        ready
    }

    pub fn slash_unbonding_stake(&mut self, validator: &str, percent: f64) -> f64 {
        let mut slashed = 0.0;
        for entry in &mut self.unbonding_queue {
            if entry.validator == validator && entry.slashable {
                let slash_amount = entry.amount * percent;
                entry.amount -= slash_amount;
                slashed += slash_amount;
            }
        }
        self.unbonding_queue.retain(|e| e.amount > 0.0);
        self.total_slashed += slashed;
        slashed
    }

    pub fn get_unbonding_queue(&self) -> &[UnbondingEntry] {
        &self.unbonding_queue
    }

    pub fn get_unbonding_for_validator(&self, validator: &str) -> Vec<&UnbondingEntry> {
        self.unbonding_queue
            .iter()
            .filter(|e| e.validator == validator)
            .collect()
    }

    pub fn get_slash_events(&self, limit: usize) -> Vec<&SlashEvent> {
        self.slash_events.iter().rev().take(limit).collect()
    }

    pub fn get_validator_slash_history(&self, validator: &str) -> Vec<&SlashEvent> {
        self.slash_events
            .iter()
            .filter(|e| e.validator == validator)
            .collect()
    }

    pub fn get_total_slashed(&self) -> f64 {
        self.total_slashed
    }

    pub fn to_json(&self) -> SlashingSnapshot {
        SlashingSnapshot {
            slash_events: self.slash_events.clone(),
            unbonding_queue: self.unbonding_queue.clone(),
            total_slashed: self.total_slashed,
            liveness_failures: self
                .liveness_failures
                .iter()
                .map(|(k, v)| (k.clone(), LivenessInfo { count: v.count, last_failure: v.last_failure }))
                .collect(),
        }
    }

    pub fn from_json(snapshot: SlashingSnapshot) -> Self {
        let next_slash_id = snapshot.slash_events.iter()
            .filter_map(|e| e.id.strip_prefix("slash_").and_then(|s| s.parse::<u64>().ok()))
            .max()
            .unwrap_or(0) + 1;

        Self {
            slash_events: snapshot.slash_events,
            unbonding_queue: snapshot.unbonding_queue,
            total_slashed: snapshot.total_slashed,
            liveness_failures: snapshot
                .liveness_failures
                .into_iter()
                .map(|(k, v)| {
                    (k, LivenessRecord { count: v.count, last_failure: v.last_failure })
                })
                .collect(),
            double_sign_evidence: Vec::new(),
            next_slash_id,
        }
    }
}

impl Default for SlashingService {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivenessInfo {
    pub count: u32,
    pub last_failure: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashingSnapshot {
    pub slash_events: Vec<SlashEvent>,
    pub unbonding_queue: Vec<UnbondingEntry>,
    pub total_slashed: f64,
    pub liveness_failures: Vec<(String, LivenessInfo)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slash_event() {
        let mut service = SlashingService::new();
        let event = service.slash("validator1", 1000.0, SlashReason::DoubleSign, 100, None);
        
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.validator, "validator1");
        assert_eq!(event.amount, 150.0);
        assert_eq!(event.percent_slashed, 15.0);
    }

    #[test]
    fn test_liveness_tracking() {
        let mut service = SlashingService::new();
        
        let result1 = service.record_liveness_failure("v1", 1, 1000.0);
        assert!(result1.is_none());
        
        let result2 = service.record_liveness_failure("v1", 2, 1000.0);
        assert!(result2.is_none());
        
        let result3 = service.record_liveness_failure("v1", 3, 1000.0);
        assert!(result3.is_some());
    }

    #[test]
    fn test_all_slash_reasons() {
        assert_eq!(SlashReason::DoubleSign.percent(), 0.15);
        assert_eq!(SlashReason::InvalidCheckpoint.percent(), 0.25);
        assert_eq!(SlashReason::InvalidProof.percent(), 0.20);
        assert_eq!(SlashReason::InvalidWitness.percent(), 0.15);
        assert_eq!(SlashReason::ReceiptTampering.percent(), 0.25);
        assert_eq!(SlashReason::LivenessFailure.percent(), 0.05);
        assert_eq!(SlashReason::LivenessRepeat.percent(), 0.10);
    }

    #[test]
    fn test_unbonding_queue() {
        let mut service = SlashingService::new();
        service.start_unbonding("v1", 500.0);
        
        assert_eq!(service.get_unbonding_queue().len(), 1);
        assert_eq!(service.get_unbonding_for_validator("v1").len(), 1);
        assert_eq!(service.get_unbonding_for_validator("v2").len(), 0);
    }

    #[test]
    fn test_double_sign_evidence_submission() {
        let mut service = SlashingService::new();
        
        let submitted = service.submit_double_sign_evidence(
            "validator1".to_string(),
            100,
            "hash1".to_string(),
            "hash2".to_string(),
            "sig1".to_string(),
            Some("sig2".to_string()),
        );
        
        assert!(submitted);
        assert_eq!(service.get_pending_evidence().len(), 1);
    }

    #[test]
    fn test_double_sign_same_hash_rejected() {
        let mut service = SlashingService::new();
        
        let submitted = service.submit_double_sign_evidence(
            "validator1".to_string(),
            100,
            "same_hash".to_string(),
            "same_hash".to_string(),
            "sig1".to_string(),
            None,
        );
        
        assert!(!submitted, "Same hash should be rejected");
        assert_eq!(service.get_pending_evidence().len(), 0);
    }

    #[test]
    fn test_double_sign_duplicate_rejected() {
        let mut service = SlashingService::new();
        
        service.submit_double_sign_evidence(
            "validator1".to_string(),
            100,
            "hash1".to_string(),
            "hash2".to_string(),
            "sig1".to_string(),
            None,
        );
        
        let duplicate = service.submit_double_sign_evidence(
            "validator1".to_string(),
            100,
            "hash1".to_string(),
            "hash2".to_string(),
            "sig1".to_string(),
            None,
        );
        
        assert!(!duplicate, "Duplicate evidence should be rejected");
        assert_eq!(service.get_pending_evidence().len(), 1);
    }

    #[test]
    fn test_process_double_sign_evidence() {
        let mut service = SlashingService::new();
        
        service.submit_double_sign_evidence(
            "validator1".to_string(),
            100,
            "hash1".to_string(),
            "hash2".to_string(),
            "sig1".to_string(),
            None,
        );
        
        let events = service.process_double_sign_evidence(1000.0);
        
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].validator, "validator1");
        assert_eq!(events[0].reason, SlashReason::DoubleSign);
        assert_eq!(events[0].amount, 150.0);
        assert_eq!(service.get_pending_evidence().len(), 0);
    }
}
