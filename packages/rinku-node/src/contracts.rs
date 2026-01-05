use anyhow::{anyhow, Result};
use rinku_core::crypto::sha256_hex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GasSchedule {
    pub base_execution: u64,
    pub storage_read: u64,
    pub storage_write: u64,
    pub storage_delete: u64,
    pub memory_alloc: u64,
    pub log: u64,
    pub emit: u64,
    pub hash: u64,
    pub balance_check: u64,
    pub account_age_check: u64,
    pub transfer: u64,
    pub mint: u64,
    pub burn: u64,
}

impl GasSchedule {
    pub fn default_schedule() -> Self {
        Self {
            base_execution: 1000,
            storage_read: 200,
            storage_write: 5000,
            storage_delete: 5000,
            memory_alloc: 3,
            log: 100,
            emit: 500,
            hash: 300,
            balance_check: 100,
            account_age_check: 100,
            transfer: 8000,
            mint: 6000,
            burn: 6000,
        }
    }

    pub fn get(&self, operation: &str) -> Option<u64> {
        match operation {
            "base_execution" => Some(self.base_execution),
            "storage_read" => Some(self.storage_read),
            "storage_write" => Some(self.storage_write),
            "storage_delete" => Some(self.storage_delete),
            "memory_alloc" => Some(self.memory_alloc),
            "log" => Some(self.log),
            "emit" => Some(self.emit),
            "hash" => Some(self.hash),
            "balance_check" => Some(self.balance_check),
            "account_age_check" => Some(self.account_age_check),
            "transfer" => Some(self.transfer),
            "mint" => Some(self.mint),
            "burn" => Some(self.burn),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GasMeter {
    gas_used: u64,
    gas_limit: u64,
    schedule: GasSchedule,
}

impl GasMeter {
    pub fn new(gas_limit: u64, schedule: GasSchedule) -> Self {
        Self {
            gas_used: 0,
            gas_limit,
            schedule,
        }
    }

    pub fn charge(&mut self, operation: &str, multiplier: u64) -> bool {
        let cost = self.schedule.get(operation).unwrap_or_else(|| {
            tracing::warn!("Unknown gas operation: {}", operation);
            0
        }) * multiplier;
        self.gas_used += cost;
        self.gas_used <= self.gas_limit
    }

    pub fn charge_custom(&mut self, amount: u64) -> bool {
        self.gas_used += amount;
        self.gas_used <= self.gas_limit
    }

    pub fn gas_used(&self) -> u64 {
        self.gas_used
    }

    pub fn gas_remaining(&self) -> u64 {
        self.gas_limit.saturating_sub(self.gas_used)
    }

    pub fn is_out_of_gas(&self) -> bool {
        self.gas_used > self.gas_limit
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractState {
    pub contract_id: String,
    pub creator: String,
    pub wasm_base64: String,
    pub deploy_url: String,
    pub state: HashMap<String, Value>,
    pub state_hash: String,
    pub height: u64,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractDeploy {
    pub contract_id: String,
    pub creator: String,
    pub wasm_base64: String,
    pub init_state: HashMap<String, Value>,
    pub ts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractCall {
    pub contract_id: String,
    pub entrypoint: String,
    pub input: HashMap<String, Value>,
    pub pre_state_hash: String,
    pub post_state_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateChange {
    pub key: String,
    pub old_value: Option<Value>,
    pub new_value: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateDiff {
    pub contract_id: String,
    pub height: u64,
    pub changes: Vec<StateChange>,
    pub pre_hash: String,
    pub post_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractEvent {
    pub contract_id: String,
    pub event_name: String,
    pub data: HashMap<String, Value>,
    pub index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionResult {
    pub success: bool,
    pub state_diff: Option<StateDiff>,
    pub gas_used: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub logs: Vec<String>,
    pub events: Vec<ContractEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractReceipt {
    pub tx_hash: String,
    pub checkpoint_height: u64,
    pub contract_id: String,
    pub entrypoint: String,
    pub caller: String,
    pub pre_state_root: String,
    pub post_state_root: String,
    pub status: String,
    pub gas_used: u64,
    pub events: Vec<ContractEvent>,
}

pub fn compute_state_hash(state: &HashMap<String, Value>) -> String {
    let mut keys: Vec<&String> = state.keys().collect();
    keys.sort();

    let mut parts = Vec::new();
    for key in keys {
        if let Some(value) = state.get(key) {
            parts.push(format!("{}:{}", key, value));
        }
    }

    let sorted = parts.join(",");
    let mut h: u32 = 0;
    for byte in sorted.bytes() {
        h = h.wrapping_shl(5).wrapping_sub(h).wrapping_add(byte as u32);
    }

    format!("{:08x}", h)
}

pub fn create_contract_id(creator: &str, nonce: u64) -> String {
    let data = format!("{}:{}", creator, nonce);
    let mut h: u32 = 0;
    for byte in data.bytes() {
        h = h.wrapping_shl(5).wrapping_sub(h).wrapping_add(byte as u32);
    }
    format!("sc_{:08x}", h)
}

pub fn compute_state_diff(
    contract_id: &str,
    height: u64,
    old_state: &HashMap<String, Value>,
    new_state: &HashMap<String, Value>,
) -> StateDiff {
    let mut changes = Vec::new();
    let mut all_keys: std::collections::HashSet<&String> = old_state.keys().collect();
    all_keys.extend(new_state.keys());

    for key in all_keys {
        let old_value = old_state.get(key).cloned();
        let new_value = new_state.get(key).cloned();

        let old_json = old_value.as_ref().map(|v| v.to_string());
        let new_json = new_value.as_ref().map(|v| v.to_string());

        if old_json != new_json {
            changes.push(StateChange {
                key: key.clone(),
                old_value,
                new_value,
            });
        }
    }

    StateDiff {
        contract_id: contract_id.to_string(),
        height,
        changes,
        pre_hash: compute_state_hash(old_state),
        post_hash: compute_state_hash(new_state),
    }
}

pub struct ContractRuntime {
    schedule: GasSchedule,
    default_gas_limit: u64,
}

impl ContractRuntime {
    pub fn new() -> Self {
        Self {
            schedule: GasSchedule::default_schedule(),
            default_gas_limit: 1_000_000,
        }
    }

    pub fn execute(
        &self,
        contract_id: &str,
        _wasm_base64: &str,
        entrypoint: &str,
        input: &HashMap<String, Value>,
        state: &HashMap<String, Value>,
        height: u64,
        gas_limit: Option<u64>,
    ) -> ExecutionResult {
        let mut meter = GasMeter::new(gas_limit.unwrap_or(self.default_gas_limit), self.schedule.clone());
        let mut logs = Vec::new();
        let mut events = Vec::new();
        let mut new_state = state.clone();

        meter.charge("base_execution", 1);

        if meter.is_out_of_gas() {
            return ExecutionResult {
                success: false,
                state_diff: None,
                gas_used: meter.gas_used(),
                error: Some("Out of gas during initialization".to_string()),
                logs,
                events,
            };
        }

        meter.charge("storage_read", 1);

        match entrypoint {
            "init" => {
                events.push(ContractEvent {
                    contract_id: contract_id.to_string(),
                    event_name: "Initialized".to_string(),
                    data: HashMap::from([("contractId".to_string(), Value::String(contract_id.to_string()))]),
                    index: 0,
                });
                meter.charge("emit", 1);

                ExecutionResult {
                    success: true,
                    state_diff: Some(compute_state_diff(contract_id, height, state, &new_state)),
                    gas_used: meter.gas_used(),
                    error: None,
                    logs,
                    events,
                }
            }

            "transfer" => {
                let from = input.get("from").and_then(|v| v.as_str()).unwrap_or_default();
                let to = input.get("to").and_then(|v| v.as_str()).unwrap_or_default();
                let amount = input.get("amount").and_then(|v| v.as_f64()).unwrap_or(0.0);

                meter.charge("balance_check", 1);

                let mut balances_map: serde_json::Map<String, Value> = new_state
                    .get("balances")
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();

                let from_balance = balances_map
                    .get(from)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);

                if from_balance < amount {
                    events.push(ContractEvent {
                        contract_id: contract_id.to_string(),
                        event_name: "TransferFailed".to_string(),
                        data: HashMap::from([
                            ("from".to_string(), Value::String(from.to_string())),
                            ("to".to_string(), Value::String(to.to_string())),
                            ("amount".to_string(), Value::Number(serde_json::Number::from_f64(amount).unwrap())),
                            ("reason".to_string(), Value::String("Insufficient balance".to_string())),
                        ]),
                        index: events.len(),
                    });
                    meter.charge("emit", 1);

                    return ExecutionResult {
                        success: false,
                        state_diff: None,
                        gas_used: meter.gas_used(),
                        error: Some("Insufficient balance".to_string()),
                        logs,
                        events,
                    };
                }

                meter.charge("transfer", 1);
                meter.charge("storage_write", 2);

                if meter.is_out_of_gas() {
                    return ExecutionResult {
                        success: false,
                        state_diff: None,
                        gas_used: meter.gas_used(),
                        error: Some("Out of gas during transfer".to_string()),
                        logs,
                        events,
                    };
                }

                balances_map.insert(from.to_string(), Value::Number(serde_json::Number::from_f64(from_balance - amount).unwrap()));
                let to_balance = balances_map.get(to).and_then(|v| v.as_f64()).unwrap_or(0.0);
                balances_map.insert(to.to_string(), Value::Number(serde_json::Number::from_f64(to_balance + amount).unwrap()));

                meter.charge("log", 1);
                logs.push(format!("Transferred {} from {} to {}", amount, from, to));

                events.push(ContractEvent {
                    contract_id: contract_id.to_string(),
                    event_name: "Transfer".to_string(),
                    data: HashMap::from([
                        ("from".to_string(), Value::String(from.to_string())),
                        ("to".to_string(), Value::String(to.to_string())),
                        ("amount".to_string(), Value::Number(serde_json::Number::from_f64(amount).unwrap())),
                    ]),
                    index: events.len(),
                });
                meter.charge("emit", 1);

                new_state.insert("balances".to_string(), Value::Object(balances_map));

                ExecutionResult {
                    success: true,
                    state_diff: Some(compute_state_diff(contract_id, height, state, &new_state)),
                    gas_used: meter.gas_used(),
                    error: None,
                    logs,
                    events,
                }
            }

            "mint" => {
                let to = input.get("to").and_then(|v| v.as_str()).unwrap_or_default();
                let amount = input.get("amount").and_then(|v| v.as_f64()).unwrap_or(0.0);

                meter.charge("mint", 1);
                meter.charge("storage_write", 1);

                if meter.is_out_of_gas() {
                    return ExecutionResult {
                        success: false,
                        state_diff: None,
                        gas_used: meter.gas_used(),
                        error: Some("Out of gas during mint".to_string()),
                        logs,
                        events,
                    };
                }

                let mut balances_map: serde_json::Map<String, Value> = new_state
                    .get("balances")
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();

                let to_balance = balances_map.get(to).and_then(|v| v.as_f64()).unwrap_or(0.0);
                balances_map.insert(to.to_string(), Value::Number(serde_json::Number::from_f64(to_balance + amount).unwrap()));
                new_state.insert("balances".to_string(), Value::Object(balances_map));

                meter.charge("log", 1);
                logs.push(format!("Minted {} to {}", amount, to));

                events.push(ContractEvent {
                    contract_id: contract_id.to_string(),
                    event_name: "Mint".to_string(),
                    data: HashMap::from([
                        ("to".to_string(), Value::String(to.to_string())),
                        ("amount".to_string(), Value::Number(serde_json::Number::from_f64(amount).unwrap())),
                    ]),
                    index: events.len(),
                });
                meter.charge("emit", 1);

                ExecutionResult {
                    success: true,
                    state_diff: Some(compute_state_diff(contract_id, height, state, &new_state)),
                    gas_used: meter.gas_used(),
                    error: None,
                    logs,
                    events,
                }
            }

            "burn" => {
                let from = input.get("from").and_then(|v| v.as_str()).unwrap_or_default();
                let amount = input.get("amount").and_then(|v| v.as_f64()).unwrap_or(0.0);

                meter.charge("balance_check", 1);

                let mut balances_map: serde_json::Map<String, Value> = new_state
                    .get("balances")
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();

                let from_balance = balances_map.get(from).and_then(|v| v.as_f64()).unwrap_or(0.0);

                if from_balance < amount {
                    events.push(ContractEvent {
                        contract_id: contract_id.to_string(),
                        event_name: "BurnFailed".to_string(),
                        data: HashMap::from([
                            ("from".to_string(), Value::String(from.to_string())),
                            ("amount".to_string(), Value::Number(serde_json::Number::from_f64(amount).unwrap())),
                            ("reason".to_string(), Value::String("Insufficient balance".to_string())),
                        ]),
                        index: events.len(),
                    });
                    meter.charge("emit", 1);

                    return ExecutionResult {
                        success: false,
                        state_diff: None,
                        gas_used: meter.gas_used(),
                        error: Some("Insufficient balance for burn".to_string()),
                        logs,
                        events,
                    };
                }

                meter.charge("burn", 1);
                meter.charge("storage_write", 1);

                if meter.is_out_of_gas() {
                    return ExecutionResult {
                        success: false,
                        state_diff: None,
                        gas_used: meter.gas_used(),
                        error: Some("Out of gas during burn".to_string()),
                        logs,
                        events,
                    };
                }

                balances_map.insert(from.to_string(), Value::Number(serde_json::Number::from_f64(from_balance - amount).unwrap()));
                new_state.insert("balances".to_string(), Value::Object(balances_map));

                meter.charge("log", 1);
                logs.push(format!("Burned {} from {}", amount, from));

                events.push(ContractEvent {
                    contract_id: contract_id.to_string(),
                    event_name: "Burn".to_string(),
                    data: HashMap::from([
                        ("from".to_string(), Value::String(from.to_string())),
                        ("amount".to_string(), Value::Number(serde_json::Number::from_f64(amount).unwrap())),
                    ]),
                    index: events.len(),
                });
                meter.charge("emit", 1);

                ExecutionResult {
                    success: true,
                    state_diff: Some(compute_state_diff(contract_id, height, state, &new_state)),
                    gas_used: meter.gas_used(),
                    error: None,
                    logs,
                    events,
                }
            }

            "get_balance" => {
                let address = input.get("address").and_then(|v| v.as_str()).unwrap_or_default();

                meter.charge("storage_read", 1);

                let balance = state
                    .get("balances")
                    .and_then(|b| b.as_object())
                    .and_then(|m| m.get(address))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);

                meter.charge("log", 1);
                logs.push(format!("Balance of {}: {}", address, balance));

                ExecutionResult {
                    success: true,
                    state_diff: None,
                    gas_used: meter.gas_used(),
                    error: None,
                    logs,
                    events,
                }
            }

            _ => {
                ExecutionResult {
                    success: false,
                    state_diff: None,
                    gas_used: meter.gas_used(),
                    error: Some(format!("Unknown entrypoint: {}", entrypoint)),
                    logs,
                    events,
                }
            }
        }
    }
}

impl Default for ContractRuntime {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ContractService {
    contracts: Arc<RwLock<HashMap<String, ContractState>>>,
    execution_history: Arc<RwLock<HashMap<String, Vec<StateDiff>>>>,
    receipts: Arc<RwLock<HashMap<String, ContractReceipt>>>,
    runtime: ContractRuntime,
    checkpoint_height: Arc<RwLock<u64>>,
}

impl ContractService {
    pub fn new() -> Self {
        Self {
            contracts: Arc::new(RwLock::new(HashMap::new())),
            execution_history: Arc::new(RwLock::new(HashMap::new())),
            receipts: Arc::new(RwLock::new(HashMap::new())),
            runtime: ContractRuntime::new(),
            checkpoint_height: Arc::new(RwLock::new(0)),
        }
    }

    pub async fn set_checkpoint_height(&self, height: u64) {
        *self.checkpoint_height.write().await = height;
    }

    pub async fn deploy_contract(&self, deploy: ContractDeploy) -> Result<(String, String)> {
        let mut contracts = self.contracts.write().await;

        if contracts.contains_key(&deploy.contract_id) {
            return Err(anyhow!("Contract ID already exists"));
        }

        let deploy_url = format!("rinku://contract/{}", deploy.contract_id);
        let state_hash = compute_state_hash(&deploy.init_state);

        let contract_state = ContractState {
            contract_id: deploy.contract_id.clone(),
            creator: deploy.creator,
            wasm_base64: deploy.wasm_base64,
            deploy_url: deploy_url.clone(),
            state: deploy.init_state,
            state_hash,
            height: 0,
            created_at: deploy.ts,
        };

        contracts.insert(deploy.contract_id.clone(), contract_state);

        let mut history = self.execution_history.write().await;
        history.insert(deploy.contract_id.clone(), Vec::new());

        info!("Contract deployed: {}", deploy.contract_id);

        Ok((deploy.contract_id, deploy_url))
    }

    pub async fn execute_call(
        &self,
        tx_hash: &str,
        caller: &str,
        call: &ContractCall,
        gas_limit: Option<u64>,
    ) -> Result<(ExecutionResult, Option<ContractReceipt>)> {
        let mut contracts = self.contracts.write().await;

        let contract = contracts
            .get_mut(&call.contract_id)
            .ok_or_else(|| anyhow!("Contract not found: {}", call.contract_id))?;

        if call.pre_state_hash != contract.state_hash {
            return Err(anyhow!(
                "Pre-state hash mismatch: expected {}, got {}",
                contract.state_hash,
                call.pre_state_hash
            ));
        }

        let pre_state_root = contract.state_hash.clone();

        let result = self.runtime.execute(
            &call.contract_id,
            &contract.wasm_base64,
            &call.entrypoint,
            &call.input,
            &contract.state,
            contract.height + 1,
            gas_limit,
        );

        if result.success {
            if let Some(ref diff) = result.state_diff {
                if diff.post_hash != call.post_state_hash {
                    return Err(anyhow!(
                        "Post-state hash mismatch: expected {}, got {}",
                        call.post_state_hash,
                        diff.post_hash
                    ));
                }

                for change in &diff.changes {
                    if let Some(ref new_value) = change.new_value {
                        contract.state.insert(change.key.clone(), new_value.clone());
                    } else {
                        contract.state.remove(&change.key);
                    }
                }

                contract.state_hash = diff.post_hash.clone();
                contract.height += 1;

                let mut history = self.execution_history.write().await;
                history
                    .entry(call.contract_id.clone())
                    .or_default()
                    .push(diff.clone());
            }

            let checkpoint_height = *self.checkpoint_height.read().await;
            let post_state_root = contract.state_hash.clone();

            let receipt = ContractReceipt {
                tx_hash: tx_hash.to_string(),
                checkpoint_height,
                contract_id: call.contract_id.clone(),
                entrypoint: call.entrypoint.clone(),
                caller: caller.to_string(),
                pre_state_root,
                post_state_root,
                status: "success".to_string(),
                gas_used: result.gas_used,
                events: result.events.clone(),
            };

            let mut receipts = self.receipts.write().await;
            receipts.insert(tx_hash.to_string(), receipt.clone());

            Ok((result, Some(receipt)))
        } else {
            Ok((result, None))
        }
    }

    pub async fn get_contract(&self, contract_id: &str) -> Option<ContractState> {
        self.contracts.read().await.get(contract_id).cloned()
    }

    pub async fn get_receipt(&self, tx_hash: &str) -> Option<ContractReceipt> {
        self.receipts.read().await.get(tx_hash).cloned()
    }

    pub async fn get_contract_count(&self) -> usize {
        self.contracts.read().await.len()
    }

    pub async fn list_contracts(&self) -> Vec<String> {
        self.contracts.read().await.keys().cloned().collect()
    }
}

impl Default for ContractService {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ContractService {
    fn clone(&self) -> Self {
        Self {
            contracts: self.contracts.clone(),
            execution_history: self.execution_history.clone(),
            receipts: self.receipts.clone(),
            runtime: ContractRuntime::new(),
            checkpoint_height: self.checkpoint_height.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gas_meter() {
        let schedule = GasSchedule::default_schedule();
        let mut meter = GasMeter::new(10000, schedule);

        assert!(meter.charge("base_execution", 1));
        assert_eq!(meter.gas_used(), 1000);
        assert!(!meter.is_out_of_gas());
    }

    #[test]
    fn test_gas_meter_out_of_gas() {
        let schedule = GasSchedule::default_schedule();
        let mut meter = GasMeter::new(100, schedule);

        meter.charge("base_execution", 1);
        assert!(meter.is_out_of_gas());
    }

    #[test]
    fn test_compute_state_hash() {
        let mut state = HashMap::new();
        state.insert("key1".to_string(), Value::String("value1".to_string()));
        state.insert("key2".to_string(), Value::Number(serde_json::Number::from(42)));

        let hash1 = compute_state_hash(&state);
        let hash2 = compute_state_hash(&state);

        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 8);
    }

    #[test]
    fn test_create_contract_id() {
        let id1 = create_contract_id("creator1", 0);
        let id2 = create_contract_id("creator1", 1);
        let id3 = create_contract_id("creator2", 0);

        assert!(id1.starts_with("sc_"));
        assert_ne!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_contract_runtime_init() {
        let runtime = ContractRuntime::new();
        let state = HashMap::new();
        let input = HashMap::new();

        let result = runtime.execute("test_contract", "", "init", &input, &state, 0, None);

        assert!(result.success);
        assert!(!result.events.is_empty());
        assert!(result.gas_used > 0);
    }

    #[test]
    fn test_contract_runtime_mint() {
        let runtime = ContractRuntime::new();
        let state = HashMap::new();
        let mut input = HashMap::new();
        input.insert("to".to_string(), Value::String("alice".to_string()));
        input.insert("amount".to_string(), Value::Number(serde_json::Number::from_f64(100.0).unwrap()));

        let result = runtime.execute("test_contract", "", "mint", &input, &state, 1, None);

        assert!(result.success);
        assert!(result.state_diff.is_some());
        assert!(!result.events.is_empty());
        assert!(result.logs.iter().any(|l| l.contains("Minted 100")));
    }

    #[test]
    fn test_contract_runtime_transfer() {
        let runtime = ContractRuntime::new();
        let mut balances = serde_json::Map::new();
        balances.insert("alice".to_string(), Value::Number(serde_json::Number::from_f64(100.0).unwrap()));

        let mut state = HashMap::new();
        state.insert("balances".to_string(), Value::Object(balances));

        let mut input = HashMap::new();
        input.insert("from".to_string(), Value::String("alice".to_string()));
        input.insert("to".to_string(), Value::String("bob".to_string()));
        input.insert("amount".to_string(), Value::Number(serde_json::Number::from_f64(50.0).unwrap()));

        let result = runtime.execute("test_contract", "", "transfer", &input, &state, 1, None);

        assert!(result.success);
        assert!(result.state_diff.is_some());
    }

    #[test]
    fn test_contract_runtime_transfer_insufficient() {
        let runtime = ContractRuntime::new();
        let state = HashMap::new();

        let mut input = HashMap::new();
        input.insert("from".to_string(), Value::String("alice".to_string()));
        input.insert("to".to_string(), Value::String("bob".to_string()));
        input.insert("amount".to_string(), Value::Number(serde_json::Number::from_f64(50.0).unwrap()));

        let result = runtime.execute("test_contract", "", "transfer", &input, &state, 1, None);

        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Insufficient balance"));
    }

    #[tokio::test]
    async fn test_contract_service_deploy() {
        let service = ContractService::new();

        let deploy = ContractDeploy {
            contract_id: "test_contract".to_string(),
            creator: "creator".to_string(),
            wasm_base64: "".to_string(),
            init_state: HashMap::new(),
            ts: 1000,
        };

        let result = service.deploy_contract(deploy).await;
        assert!(result.is_ok());

        let (contract_id, deploy_url) = result.unwrap();
        assert_eq!(contract_id, "test_contract");
        assert!(deploy_url.starts_with("rinku://contract/"));
    }
}
