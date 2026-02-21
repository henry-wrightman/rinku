#[cfg(feature = "wasm")]
use crate::wasm_runtime;
use anyhow::{anyhow, Result};
use rinku_core::encoding::{encode_to_url, create_receipt_url, create_verifiable_object_url};
use rinku_core::stateful_receipt::{
    CheckpointFinality, ContractEventCompact, ContractSchema, MultiProof,
    ProofInput, ReceiptStatus, StatefulContractCall,
    StatefulContractDeploy, StatefulReceipt, ValidatedProofContext, VerifiableObject,
    ViewKeyProof, ViewKeyValue,
    compute_view_key_leaf_hash,
};
use rinku_core::merkle::MerkleTree;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<ContractSchema>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transfer_effects: Vec<TransferEffect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub view_key_values: Vec<ViewKeyEffect>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferEffect {
    pub from: String,
    pub to: String,
    pub amount: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewKeyEffect {
    pub key: String,
    pub value: Value,
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
    use sha2::{Sha256, Digest};
    let data = format!("rinku:contract:{}:{}", creator, nonce);
    let hash = Sha256::digest(data.as_bytes());
    let hex_str: String = hash[..20].iter().map(|b| format!("{:02x}", b)).collect();
    format!("sc_{}", hex_str)
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
    #[cfg(feature = "wasm")]
    wasm_engine: wasm_runtime::WasmEngine,
}

impl ContractRuntime {
    pub fn new() -> Self {
        Self {
            schedule: GasSchedule::default_schedule(),
            default_gas_limit: 1_000_000,
            #[cfg(feature = "wasm")]
            wasm_engine: wasm_runtime::WasmEngine::new(),
        }
    }

    pub fn execute(
        &self,
        contract_id: &str,
        wasm_base64: &str,
        entrypoint: &str,
        input: &HashMap<String, Value>,
        state: &HashMap<String, Value>,
        height: u64,
        gas_limit: Option<u64>,
    ) -> ExecutionResult {
        #[cfg(feature = "wasm")]
        {
            if let Some(wasm_bytes) = self.try_decode_wasm(wasm_base64) {
                if wasm_runtime::is_valid_wasm(&wasm_bytes) {
                    info!("Executing contract {} via WASM runtime (entrypoint: {})", contract_id, entrypoint);
                    let output = self.wasm_engine.execute(
                        contract_id,
                        &wasm_bytes,
                        entrypoint,
                        input,
                        state,
                        height,
                        gas_limit,
                        "",
                        0,
                    );
                    return output.result;
                }
            }
        }
        self.execute_mock(contract_id, entrypoint, input, state, height, gas_limit)
    }

    #[cfg(feature = "wasm")]
    fn try_decode_wasm(&self, wasm_base64: &str) -> Option<Vec<u8>> {
        if wasm_base64.is_empty() {
            return None;
        }
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(wasm_base64)
            .ok()
    }

    pub fn execute_with_caller(
        &self,
        contract_id: &str,
        wasm_base64: &str,
        entrypoint: &str,
        input: &HashMap<String, Value>,
        state: &HashMap<String, Value>,
        height: u64,
        gas_limit: Option<u64>,
        caller: &str,
        timestamp: u64,
    ) -> ExecutionResult {
        #[cfg(feature = "wasm")]
        {
            if let Some(wasm_bytes) = self.try_decode_wasm(wasm_base64) {
                if wasm_runtime::is_valid_wasm(&wasm_bytes) {
                    info!("Executing contract {} via WASM runtime (entrypoint: {}, caller: {})", contract_id, entrypoint, caller);
                    let output = self.wasm_engine.execute(
                        contract_id,
                        &wasm_bytes,
                        entrypoint,
                        input,
                        state,
                        height,
                        gas_limit,
                        caller,
                        timestamp,
                    );
                    return output.result;
                }
            }
        }
        self.execute_mock(contract_id, entrypoint, input, state, height, gas_limit)
    }

    #[cfg(feature = "wasm")]
    pub fn execute_full(
        &self,
        contract_id: &str,
        wasm_base64: &str,
        entrypoint: &str,
        input: &HashMap<String, Value>,
        state: &HashMap<String, Value>,
        height: u64,
        gas_limit: Option<u64>,
        caller: &str,
        timestamp: u64,
        ctx: wasm_runtime::ExecutionContext,
    ) -> wasm_runtime::WasmExecutionOutput {
        if let Some(wasm_bytes) = self.try_decode_wasm(wasm_base64) {
            if wasm_runtime::is_valid_wasm(&wasm_bytes) {
                info!("Executing contract {} via WASM runtime with full context (entrypoint: {}, caller: {})", contract_id, entrypoint, caller);
                return self.wasm_engine.execute_with_context(
                    contract_id,
                    &wasm_bytes,
                    entrypoint,
                    input,
                    state,
                    height,
                    gas_limit,
                    caller,
                    timestamp,
                    ctx,
                );
            }
        }
        let result = self.execute_mock(contract_id, entrypoint, input, state, height, gas_limit);
        wasm_runtime::WasmExecutionOutput {
            result,
            transfer_ops: Vec::new(),
            view_key_emissions: Vec::new(),
            return_data: Vec::new(),
        }
    }

    fn execute_mock(
        &self,
        contract_id: &str,
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
            schema: None,
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
                let contract_history = history
                    .entry(call.contract_id.clone())
                    .or_default();
                contract_history.push(diff.clone());
                
                const MAX_HISTORY_PER_CONTRACT: usize = 1000;
                if contract_history.len() > MAX_HISTORY_PER_CONTRACT {
                    contract_history.drain(0..contract_history.len() - MAX_HISTORY_PER_CONTRACT);
                }
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
                transfer_effects: Vec::new(),
                view_key_values: Vec::new(),
            };

            let mut receipts = self.receipts.write().await;
            receipts.insert(tx_hash.to_string(), receipt.clone());
            
            const MAX_RECEIPTS: usize = 10000;
            if receipts.len() > MAX_RECEIPTS {
                let keys_to_remove: Vec<_> = receipts.keys().take(MAX_RECEIPTS / 2).cloned().collect();
                for key in keys_to_remove {
                    receipts.remove(&key);
                }
            }

            Ok((result, Some(receipt)))
        } else {
            Ok((result, None))
        }
    }

    #[cfg(feature = "wasm")]
    pub async fn execute_call_with_effects(
        &self,
        tx_hash: &str,
        caller: &str,
        call: &ContractCall,
        gas_limit: Option<u64>,
        account_snapshots: HashMap<String, wasm_runtime::AccountSnapshot>,
        timestamp: u64,
    ) -> Result<(ExecutionResult, Option<ContractReceipt>, Vec<TransferEffect>, Vec<ViewKeyEffect>)> {
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

        let mut ctx = wasm_runtime::ExecutionContext::default();
        ctx.accounts = account_snapshots;

        let output = self.runtime.execute_full(
            &call.contract_id,
            &contract.wasm_base64,
            &call.entrypoint,
            &call.input,
            &contract.state,
            contract.height + 1,
            gas_limit,
            caller,
            timestamp,
            ctx,
        );

        let transfer_effects: Vec<TransferEffect> = output.transfer_ops.iter().map(|op| {
            TransferEffect {
                from: op.from.clone(),
                to: op.to.clone(),
                amount: op.amount,
            }
        }).collect();

        let view_key_effects: Vec<ViewKeyEffect> = output.view_key_emissions.iter().map(|vk| {
            ViewKeyEffect {
                key: vk.key.clone(),
                value: vk.value.clone(),
            }
        }).collect();

        let result = output.result;

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
                let contract_history = history
                    .entry(call.contract_id.clone())
                    .or_default();
                contract_history.push(diff.clone());

                const MAX_HISTORY_PER_CONTRACT: usize = 1000;
                if contract_history.len() > MAX_HISTORY_PER_CONTRACT {
                    contract_history.drain(0..contract_history.len() - MAX_HISTORY_PER_CONTRACT);
                }
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
                transfer_effects: transfer_effects.clone(),
                view_key_values: view_key_effects.clone(),
            };

            let mut receipts = self.receipts.write().await;
            receipts.insert(tx_hash.to_string(), receipt.clone());

            const MAX_RECEIPTS: usize = 10000;
            if receipts.len() > MAX_RECEIPTS {
                let keys_to_remove: Vec<_> = receipts.keys().take(MAX_RECEIPTS / 2).cloned().collect();
                for key in keys_to_remove {
                    receipts.remove(&key);
                }
            }

            Ok((result, Some(receipt), transfer_effects, view_key_effects))
        } else {
            Ok((result, None, Vec::new(), Vec::new()))
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

    pub async fn deploy_stateful_contract(&self, deploy: StatefulContractDeploy) -> Result<(String, String)> {
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
            schema: Some(deploy.schema),
        };

        contracts.insert(deploy.contract_id.clone(), contract_state);

        let mut history = self.execution_history.write().await;
        history.insert(deploy.contract_id.clone(), Vec::new());

        info!("Stateful contract deployed: {} with view key schema", deploy.contract_id);

        Ok((deploy.contract_id, deploy_url))
    }

    pub async fn execute_stateful_call(
        &self,
        tx_hash: &str,
        caller: &str,
        call: &StatefulContractCall,
        gas_limit: Option<u64>,
        chain_id: &str,
        finality: CheckpointFinality,
    ) -> Result<(ExecutionResult, Option<StatefulReceipt>)> {
        let validated_proofs = self.validate_proof_inputs(&call.proof_inputs)?;

        let mut merged_input = call.input.clone();
        for ctx in &validated_proofs {
            for (k, v) in &ctx.extracted_values {
                merged_input.insert(format!("proof.{}.{}", ctx.label, k), v.clone());
            }
        }
        merged_input.insert("_proof_context".to_string(), serde_json::to_value(&validated_proofs).unwrap_or(Value::Null));

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
            &merged_input,
            &contract.state,
            contract.height + 1,
            gas_limit,
        );

        if result.success {
            if let Some(ref diff) = result.state_diff {
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
                let contract_history = history.entry(call.contract_id.clone()).or_default();
                contract_history.push(diff.clone());

                const MAX_HISTORY_PER_CONTRACT: usize = 1000;
                if contract_history.len() > MAX_HISTORY_PER_CONTRACT {
                    contract_history.drain(0..contract_history.len() - MAX_HISTORY_PER_CONTRACT);
                }
            }

            let post_state_root = contract.state_hash.clone();

            let (view_keys, multi_proof) = self.extract_view_keys_with_proofs(
                &call.contract_id,
                &contract.state,
                contract.schema.as_ref(),
            );

            let compact_events: Vec<ContractEventCompact> = result.events.iter().map(|e| {
                ContractEventCompact {
                    name: e.event_name.clone(),
                    data: e.data.clone(),
                    index: e.index,
                }
            }).collect();

            let mut stateful_receipt = StatefulReceipt {
                version: 1,
                tx_hash: tx_hash.to_string(),
                chain_id: chain_id.to_string(),
                contract_id: call.contract_id.clone(),
                entrypoint: call.entrypoint.clone(),
                caller: caller.to_string(),
                pre_state_root,
                post_state_root,
                view_keys,
                multi_proof,
                finality,
                events: compact_events,
                gas_used: result.gas_used,
                status: ReceiptStatus::Success,
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                self_proof_url: None,
            };

            if let Ok(encoded) = encode_to_url(&stateful_receipt) {
                stateful_receipt.self_proof_url = Some(create_receipt_url(&encoded));
            }

            let legacy_receipt = ContractReceipt {
                tx_hash: tx_hash.to_string(),
                checkpoint_height: stateful_receipt.finality.checkpoint_height,
                contract_id: call.contract_id.clone(),
                entrypoint: call.entrypoint.clone(),
                caller: caller.to_string(),
                pre_state_root: stateful_receipt.pre_state_root.clone(),
                post_state_root: stateful_receipt.post_state_root.clone(),
                status: "success".to_string(),
                gas_used: result.gas_used,
                events: result.events.clone(),
                transfer_effects: Vec::new(),
                view_key_values: Vec::new(),
            };

            let mut receipts = self.receipts.write().await;
            receipts.insert(tx_hash.to_string(), legacy_receipt);

            const MAX_RECEIPTS: usize = 10000;
            if receipts.len() > MAX_RECEIPTS {
                let keys_to_remove: Vec<_> = receipts.keys().take(MAX_RECEIPTS / 2).cloned().collect();
                for key in keys_to_remove {
                    receipts.remove(&key);
                }
            }

            Ok((result, Some(stateful_receipt)))
        } else {
            Ok((result, None))
        }
    }

    fn validate_proof_inputs(&self, proof_inputs: &[ProofInput]) -> Result<Vec<ValidatedProofContext>> {
        let mut contexts = Vec::with_capacity(proof_inputs.len());

        for input in proof_inputs {
            input.validate().map_err(|e| anyhow!("Proof validation failed for '{}': {}", input.label, e))?;
            contexts.push(ValidatedProofContext::from_proof_input(input));
        }

        Ok(contexts)
    }

    fn extract_view_keys_with_proofs(
        &self,
        contract_id: &str,
        state: &HashMap<String, Value>,
        schema: Option<&ContractSchema>,
    ) -> (Vec<ViewKeyValue>, Option<MultiProof>) {
        let specs = match schema {
            Some(s) => &s.view_keys,
            None => return (vec![], None),
        };

        let raw_keys: Vec<(String, Value, String)> = specs.iter().filter_map(|spec| {
            let value = resolve_path(state, &spec.path)?;
            let leaf_hash = compute_view_key_leaf_hash(contract_id, &spec.key, &value);
            Some((spec.key.clone(), value, leaf_hash))
        }).collect();

        if raw_keys.is_empty() {
            return (vec![], None);
        }

        let leaf_hashes: Vec<[u8; 32]> = raw_keys.iter().filter_map(|(_, _, hash)| {
            let bytes = hex::decode(hash).ok()?;
            if bytes.len() != 32 { return None; }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Some(arr)
        }).collect();

        if leaf_hashes.is_empty() {
            let view_keys = raw_keys.into_iter().map(|(key, value, leaf_hash)| {
                ViewKeyValue {
                    key, value, leaf_hash,
                    proof: ViewKeyProof { siblings: vec![], path_bits: vec![], root: String::new() },
                }
            }).collect();
            return (view_keys, None);
        }

        match MerkleTree::new(leaf_hashes) {
            Ok(tree) => {
                let root = tree.root();
                let mut all_siblings = Vec::new();
                let mut path_bitmap = Vec::new();

                let view_keys: Vec<ViewKeyValue> = raw_keys.into_iter().enumerate().map(|(i, (key, value, leaf_hash))| {
                    let proof = tree.get_proof(i).ok();
                    if let Some(ref p) = proof {
                        all_siblings.extend(p.siblings.clone());
                        for bit in &p.path_bits {
                            path_bitmap.push(if *bit { 1u8 } else { 0u8 });
                        }
                    }

                    ViewKeyValue {
                        key, value, leaf_hash,
                        proof: ViewKeyProof {
                            siblings: proof.as_ref().map(|p| p.siblings.clone()).unwrap_or_default(),
                            path_bits: proof.as_ref().map(|p| p.path_bits.clone()).unwrap_or_default(),
                            root: root.clone(),
                        },
                    }
                }).collect();

                let multi_proof = MultiProof {
                    keys: view_keys.iter().map(|vk| vk.key.clone()).collect(),
                    leaf_hashes: view_keys.iter().map(|vk| vk.leaf_hash.clone()).collect(),
                    shared_siblings: all_siblings,
                    path_bitmap,
                    root,
                };

                (view_keys, Some(multi_proof))
            }
            Err(_) => {
                let view_keys = raw_keys.into_iter().map(|(key, value, leaf_hash)| {
                    ViewKeyValue {
                        key, value, leaf_hash,
                        proof: ViewKeyProof { siblings: vec![], path_bits: vec![], root: String::new() },
                    }
                }).collect();
                (view_keys, None)
            }
        }
    }

    pub fn create_verifiable_object_from_receipt(receipt: &StatefulReceipt) -> Result<String> {
        let vo = VerifiableObject::ContractOutput {
            receipt: receipt.clone(),
        };
        let encoded = encode_to_url(&vo).map_err(|e| anyhow!("Encoding failed: {}", e))?;
        Ok(create_verifiable_object_url(&encoded))
    }
}

fn resolve_path(state: &HashMap<String, Value>, path: &str) -> Option<Value> {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return None;
    }

    let mut current: Value = state.get(parts[0])?.clone();

    for &part in &parts[1..] {
        current = match current {
            Value::Object(map) => map.get(part)?.clone(),
            Value::Array(arr) => {
                let idx: usize = part.parse().ok()?;
                arr.get(idx)?.clone()
            }
            _ => return None,
        };
    }

    Some(current)
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
        assert_eq!(id1.len(), 3 + 40);
        assert_ne!(id1, id2);
        assert_ne!(id1, id3);

        let id_repeat = create_contract_id("creator1", 0);
        assert_eq!(id1, id_repeat);
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
