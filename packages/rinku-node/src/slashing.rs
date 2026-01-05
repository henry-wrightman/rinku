use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use tracing::warn;

pub const UNBONDING_PERIOD_MS: u64 = 14 * 24 * 60 * 60 * 1000;
pub const SLASH_DOUBLE_SIGN_PERCENT: f64 = 0.15;
pub const SLASH_INVALID_CHECKPOINT_PERCENT: f64 = 0.25;
pub const SLASH_INVALID_PROOF_PERCENT: f64 = 0.20;
pub const SLASH_LIVENESS_PERCENT: f64 = 0.05;
pub const SLASH_LIVENESS_REPEAT_PERCENT: f64 = 0.10;
pub const LIVENESS_MISS_THRESHOLD: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlashReason {
    DoubleSign,
    InvalidCheckpoint,
    InvalidProof,
    LivenessFailure,
    LivenessRepeat,
}

impl SlashReason {
    pub fn percent(&self) -> f64 {
        match self {
            SlashReason::DoubleSign => SLASH_DOUBLE_SIGN_PERCENT,
            SlashReason::InvalidCheckpoint => SLASH_INVALID_CHECKPOINT_PERCENT,
            SlashReason::InvalidProof => SLASH_INVALID_PROOF_PERCENT,
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

#[derive(Debug, Clone)]
struct LivenessRecord {
    count: u32,
    last_failure: u64,
}

pub struct SlashingService {
    slash_events: Vec<SlashEvent>,
    unbonding_queue: Vec<UnbondingEntry>,
    liveness_failures: HashMap<String, LivenessRecord>,
    total_slashed: f64,
}

impl SlashingService {
    pub fn new() -> Self {
        Self {
            slash_events: Vec::new(),
            unbonding_queue: Vec::new(),
            liveness_failures: HashMap::new(),
            total_slashed: 0.0,
        }
    }

    pub fn slash(
        &mut self,
        validator: &str,
        stake_amount: f64,
        reason: SlashReason,
        checkpoint_height: u64,
        details: Option<String>,
    ) -> SlashEvent {
        let percent = reason.percent();
        let amount = stake_amount * percent;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let event = SlashEvent {
            id: format!("slash-{}-{}", validator, now),
            validator: validator.to_string(),
            reason,
            amount,
            percent_slashed: percent * 100.0,
            checkpoint_height,
            timestamp: now,
            details,
        };

        self.total_slashed += amount;
        self.slash_events.push(event.clone());

        warn!(
            "Slashed {} for {:?}: {} RKU ({}%)",
            validator,
            reason,
            amount,
            percent * 100.0
        );

        event
    }

    pub fn record_liveness_failure(&mut self, validator: &str) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let record = self
            .liveness_failures
            .entry(validator.to_string())
            .or_insert(LivenessRecord {
                count: 0,
                last_failure: now,
            });

        record.count += 1;
        record.last_failure = now;

        record.count >= LIVENESS_MISS_THRESHOLD
    }

    pub fn clear_liveness_failures(&mut self, validator: &str) {
        self.liveness_failures.remove(validator);
    }

    pub fn add_to_unbonding_queue(&mut self, validator: &str, amount: f64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let entry = UnbondingEntry {
            validator: validator.to_string(),
            amount,
            started_at: now,
            available_at: now + UNBONDING_PERIOD_MS,
            slashable: true,
        };

        self.unbonding_queue.push(entry);
    }

    pub fn process_unbonding_queue(&mut self) -> Vec<UnbondingEntry> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let (ready, pending): (Vec<_>, Vec<_>) = self
            .unbonding_queue
            .drain(..)
            .partition(|e| e.available_at <= now);

        self.unbonding_queue = pending;
        ready
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
                .map(|(k, v)| (k.clone(), (v.count, v.last_failure)))
                .collect(),
        }
    }

    pub fn from_json(snapshot: SlashingSnapshot) -> Self {
        Self {
            slash_events: snapshot.slash_events,
            unbonding_queue: snapshot.unbonding_queue,
            total_slashed: snapshot.total_slashed,
            liveness_failures: snapshot
                .liveness_failures
                .into_iter()
                .map(|(k, (count, last_failure))| {
                    (k, LivenessRecord { count, last_failure })
                })
                .collect(),
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
pub struct SlashingSnapshot {
    pub slash_events: Vec<SlashEvent>,
    pub unbonding_queue: Vec<UnbondingEntry>,
    pub total_slashed: f64,
    pub liveness_failures: Vec<(String, (u32, u64))>,
}
