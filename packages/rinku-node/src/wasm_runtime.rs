use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tracing::info;
use wasmi::*;

use crate::contracts::{
    compute_state_diff, ContractEvent, ExecutionResult, GasMeter,
    GasSchedule,
};

const MAX_STORAGE_KEY_LEN: usize = 256;
const MAX_STORAGE_VALUE_LEN: usize = 8192;
const MAX_LOG_LEN: usize = 1024;
const MAX_LOGS: usize = 64;
const MAX_EVENTS: usize = 64;
const MAX_VIEW_KEYS: usize = 32;
const MAX_TRANSFERS: usize = 16;
const DEFAULT_FUEL: u64 = 10_000_000;
const MAX_MEMORY_PAGES: u32 = 256;
const MAX_RETURN_DATA: usize = 65536;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmAbi {
    pub version: u32,
    pub entrypoints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountSnapshot {
    pub address: String,
    pub balance: f64,
    pub nonce: u64,
    pub first_seen: u64,
    pub staked: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferOp {
    pub from: String,
    pub to: String,
    pub amount: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewKeyEmission {
    pub key: String,
    pub value: Value,
}

#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub accounts: HashMap<String, AccountSnapshot>,
    pub gas_price: f64,
    pub max_memory_pages: u32,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            accounts: HashMap::new(),
            gas_price: 1.0,
            max_memory_pages: MAX_MEMORY_PAGES,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmExecutionOutput {
    pub result: ExecutionResult,
    pub transfer_ops: Vec<TransferOp>,
    pub view_key_emissions: Vec<ViewKeyEmission>,
    pub return_data: Vec<u8>,
}

pub struct HostState {
    pub contract_id: String,
    pub caller: String,
    pub block_height: u64,
    pub timestamp: u64,
    pub storage: HashMap<String, Value>,
    pub original_storage: HashMap<String, Value>,
    pub logs: Vec<String>,
    pub events: Vec<ContractEvent>,
    pub return_data: Vec<u8>,
    pub gas_meter: GasMeter,
    pub input_data: Vec<u8>,
    pub error: Option<String>,
    alloc_offset: u32,
    pub accounts: HashMap<String, AccountSnapshot>,
    pub transfer_ops: Vec<TransferOp>,
    pub view_key_emissions: Vec<ViewKeyEmission>,
    pub max_memory_pages: u32,
}

impl HostState {
    pub fn new(
        contract_id: String,
        caller: String,
        block_height: u64,
        timestamp: u64,
        storage: HashMap<String, Value>,
        input: &HashMap<String, Value>,
        gas_limit: u64,
        ctx: ExecutionContext,
    ) -> Self {
        let original_storage = storage.clone();
        let input_data = serde_json::to_vec(input).unwrap_or_default();
        Self {
            contract_id,
            caller,
            block_height,
            timestamp,
            storage,
            original_storage,
            logs: Vec::new(),
            events: Vec::new(),
            return_data: Vec::new(),
            gas_meter: GasMeter::new(gas_limit, GasSchedule::default_schedule()),
            input_data,
            error: None,
            alloc_offset: 0,
            accounts: ctx.accounts,
            transfer_ops: Vec::new(),
            view_key_emissions: Vec::new(),
            max_memory_pages: ctx.max_memory_pages,
        }
    }
}

fn get_memory(caller: &Caller<'_, HostState>) -> Option<Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(mem)) => Some(mem),
        _ => None,
    }
}

fn write_to_guest(
    memory: &Memory,
    caller: &mut Caller<'_, HostState>,
    data: &[u8],
) -> Result<(i32, i32), Error> {
    let offset = caller.data().alloc_offset;
    let len = data.len() as u32;
    let mem_size = memory.data_size(caller.as_context()) as u32;
    let max_bytes = caller.data().max_memory_pages as u32 * 65536;

    if offset + len > max_bytes {
        return Err(Error::new("Memory limit exceeded"));
    }

    if offset + len > mem_size {
        let needed_pages = ((offset + len - mem_size) as u64 + 65535) / 65536;
        let current_pages = mem_size / 65536;
        if current_pages + needed_pages as u32 > caller.data().max_memory_pages {
            return Err(Error::new("Memory page limit exceeded"));
        }
        memory.grow(caller.as_context_mut(), needed_pages as u64).map_err(|_| {
            Error::new("Memory grow failed")
        })?;
    }

    memory.write(caller.as_context_mut(), offset as usize, data).map_err(|_| {
        Error::new("Memory write failed")
    })?;

    caller.data_mut().alloc_offset = offset + len;
    Ok((offset as i32, len as i32))
}

fn read_from_guest(
    memory: &Memory,
    caller: &Caller<'_, HostState>,
    ptr: i32,
    len: i32,
) -> Result<Vec<u8>, Error> {
    if ptr < 0 || len < 0 {
        return Err(Error::new("Invalid memory pointer or length"));
    }
    let ptr = ptr as usize;
    let len = len as usize;
    let mem_size = memory.data_size(caller.as_context()) as usize;

    if ptr + len > mem_size {
        return Err(Error::new("Memory access out of bounds"));
    }

    let mut buf = vec![0u8; len];
    memory.read(caller.as_context(), ptr, &mut buf).map_err(|_| {
        Error::new("Memory read failed")
    })?;
    Ok(buf)
}

fn read_string_from_guest(
    memory: &Memory,
    caller: &Caller<'_, HostState>,
    ptr: i32,
    len: i32,
    max_len: usize,
) -> Result<String, Error> {
    let bytes = read_from_guest(memory, caller, ptr, len)?;
    if bytes.len() > max_len {
        return Err(Error::new("String too long"));
    }
    String::from_utf8(bytes).map_err(|_| Error::new("Invalid UTF-8"))
}

pub struct WasmEngine {
    engine: Engine,
}

impl WasmEngine {
    pub fn new() -> Self {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        Self { engine }
    }

    pub fn validate_wasm(&self, wasm_bytes: &[u8]) -> Result<()> {
        let module = Module::new(&self.engine, wasm_bytes)
            .map_err(|e| anyhow!("Invalid WASM module: {}", e))?;

        for import in module.imports() {
            if import.module() != "rinku" && import.module() != "env" {
                return Err(anyhow!("Module imports from unknown namespace '{}', only 'rinku' and 'env' are allowed", import.module()));
            }
        }

        Ok(())
    }

    pub fn execute(
        &self,
        contract_id: &str,
        wasm_bytes: &[u8],
        entrypoint: &str,
        input: &HashMap<String, Value>,
        state: &HashMap<String, Value>,
        height: u64,
        gas_limit: Option<u64>,
        caller: &str,
        timestamp: u64,
    ) -> WasmExecutionOutput {
        self.execute_with_context(
            contract_id,
            wasm_bytes,
            entrypoint,
            input,
            state,
            height,
            gas_limit,
            caller,
            timestamp,
            ExecutionContext::default(),
        )
    }

    pub fn execute_with_context(
        &self,
        contract_id: &str,
        wasm_bytes: &[u8],
        entrypoint: &str,
        input: &HashMap<String, Value>,
        state: &HashMap<String, Value>,
        height: u64,
        gas_limit: Option<u64>,
        caller: &str,
        timestamp: u64,
        ctx: ExecutionContext,
    ) -> WasmExecutionOutput {
        let fuel = gas_limit.unwrap_or(DEFAULT_FUEL);
        let host_state = HostState::new(
            contract_id.to_string(),
            caller.to_string(),
            height,
            timestamp,
            state.clone(),
            input,
            fuel,
            ctx,
        );

        let mut store = Store::new(&self.engine, host_state);
        if let Err(e) = store.set_fuel(fuel) {
            return wrap_err(format!("Failed to set fuel: {}", e));
        }

        let module = match Module::new(&self.engine, wasm_bytes) {
            Ok(m) => m,
            Err(e) => return wrap_err(format!("Failed to compile WASM: {}", e)),
        };

        let mut linker = <Linker<HostState>>::new(&self.engine);
        if let Err(e) = self.register_host_functions(&mut linker) {
            return wrap_err(format!("Failed to register host functions: {}", e));
        }

        let instance = match linker.instantiate_and_start(&mut store, &module) {
            Ok(inst) => inst,
            Err(e) => {
                let consumed = fuel.saturating_sub(store.get_fuel().unwrap_or(0));
                let host = store.into_data();
                return WasmExecutionOutput {
                    result: ExecutionResult {
                        success: false,
                        state_diff: None,
                        gas_used: consumed,
                        error: Some(format!("WASM instantiation failed: {}", e)),
                        logs: host.logs,
                        events: Vec::new(),
                    },
                    transfer_ops: Vec::new(),
                    view_key_emissions: Vec::new(),
                    return_data: Vec::new(),
                };
            }
        };

        if let Some(Extern::Memory(mem)) = instance.get_export(&store, "memory") {
            let mem_size = mem.data_size(&store) as u32;
            if mem_size > 0 {
                store.data_mut().alloc_offset = mem_size;
                tracing::info!(
                    "WASM alloc_offset={} (mem_size={}, pages={}) for {} entrypoint={}",
                    mem_size, mem_size, mem_size / 65536, contract_id, entrypoint
                );
            }
        }

        let result_i32 = instance.get_typed_func::<(), i32>(&store, entrypoint);
        let result_void = instance.get_typed_func::<(), ()>(&store, entrypoint);

        if let Ok(func) = result_i32 {
            let call_result = func.call(&mut store, ());
            let consumed = fuel.saturating_sub(store.get_fuel().unwrap_or(0));
            return self.handle_i32_result(call_result, store, contract_id, height, consumed);
        }

        if let Ok(func) = result_void {
            let call_result = func.call(&mut store, ());
            let consumed = fuel.saturating_sub(store.get_fuel().unwrap_or(0));
            return self.handle_void_result(call_result, store, contract_id, height, consumed);
        }

        wrap_err(format!("Entrypoint '{}' not found in WASM module", entrypoint))
    }

    fn handle_i32_result(
        &self,
        call_result: Result<i32, Error>,
        store: Store<HostState>,
        contract_id: &str,
        height: u64,
        consumed: u64,
    ) -> WasmExecutionOutput {
        match call_result {
            Ok(status_code) => {
                let host = store.into_data();
                if status_code != 0 {
                    return WasmExecutionOutput {
                        transfer_ops: Vec::new(),
                        view_key_emissions: Vec::new(),
                        return_data: Vec::new(),
                        result: ExecutionResult {
                            success: false,
                            state_diff: None,
                            gas_used: consumed,
                            error: host.error.or_else(|| Some(format!("Contract returned error code: {}", status_code))),
                            logs: host.logs,
                            events: host.events,
                        },
                    };
                }
                build_success_output(host, contract_id, height, consumed)
            }
            Err(e) => {
                let host = store.into_data();
                WasmExecutionOutput {
                    transfer_ops: Vec::new(),
                    view_key_emissions: Vec::new(),
                    return_data: Vec::new(),
                    result: ExecutionResult {
                        success: false,
                        state_diff: None,
                        gas_used: consumed,
                        error: Some(host.error.unwrap_or_else(|| format!("Execution error: {}", e))),
                        logs: host.logs,
                        events: host.events,
                    },
                }
            }
        }
    }

    fn handle_void_result(
        &self,
        call_result: Result<(), Error>,
        store: Store<HostState>,
        contract_id: &str,
        height: u64,
        consumed: u64,
    ) -> WasmExecutionOutput {
        match call_result {
            Ok(()) => {
                let host = store.into_data();
                build_success_output(host, contract_id, height, consumed)
            }
            Err(e) => {
                let host = store.into_data();
                WasmExecutionOutput {
                    transfer_ops: Vec::new(),
                    view_key_emissions: Vec::new(),
                    return_data: Vec::new(),
                    result: ExecutionResult {
                        success: false,
                        state_diff: None,
                        gas_used: consumed,
                        error: Some(host.error.unwrap_or_else(|| format!("Execution error: {}", e))),
                        logs: host.logs,
                        events: host.events,
                    },
                }
            }
        }
    }

    fn register_host_functions(&self, linker: &mut Linker<HostState>) -> Result<()> {
        for module in &["rinku", "env"] {
            self.register_storage_functions(linker, module)?;
            self.register_context_functions(linker, module)?;
            self.register_io_functions(linker, module)?;
            self.register_crypto_functions(linker, module)?;
            self.register_ledger_functions(linker, module)?;
        }
        Ok(())
    }

    fn register_storage_functions(&self, linker: &mut Linker<HostState>, module: &str) -> Result<()> {
        linker.func_wrap(module, "storage_read", |mut caller: Caller<'_, HostState>, key_ptr: i32, key_len: i32| -> i32 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let key = match read_string_from_guest(&memory, &caller, key_ptr, key_len, MAX_STORAGE_KEY_LEN) {
                Ok(k) => k,
                Err(_) => return -1,
            };

            caller.data_mut().gas_meter.charge("storage_read", 1);

            let value = match caller.data().storage.get(&key) {
                Some(v) => serde_json::to_vec(v).unwrap_or_default(),
                None => return 0,
            };

            match write_to_guest(&memory, &mut caller, &value) {
                Ok((ptr, _len)) => ptr,
                Err(_) => -1,
            }
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "storage_read_len", |caller: Caller<'_, HostState>, key_ptr: i32, key_len: i32| -> i32 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let key = match read_string_from_guest(&memory, &caller, key_ptr, key_len, MAX_STORAGE_KEY_LEN) {
                Ok(k) => k,
                Err(_) => return -1,
            };

            match caller.data().storage.get(&key) {
                Some(v) => serde_json::to_vec(v).unwrap_or_default().len() as i32,
                None => 0,
            }
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "storage_write", |mut caller: Caller<'_, HostState>, key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32| -> i32 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let key = match read_string_from_guest(&memory, &caller, key_ptr, key_len, MAX_STORAGE_KEY_LEN) {
                Ok(k) => k,
                Err(_) => return -1,
            };

            let val_bytes = match read_from_guest(&memory, &caller, val_ptr, val_len) {
                Ok(b) if b.len() <= MAX_STORAGE_VALUE_LEN => b,
                _ => return -1,
            };

            caller.data_mut().gas_meter.charge("storage_write", 1);

            let value: Value = match serde_json::from_slice(&val_bytes) {
                Ok(v) => v,
                Err(_) => return -1,
            };

            caller.data_mut().storage.insert(key, value);
            0
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "storage_delete", |mut caller: Caller<'_, HostState>, key_ptr: i32, key_len: i32| -> i32 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let key = match read_string_from_guest(&memory, &caller, key_ptr, key_len, MAX_STORAGE_KEY_LEN) {
                Ok(k) => k,
                Err(_) => return -1,
            };

            caller.data_mut().gas_meter.charge("storage_delete", 1);

            match caller.data_mut().storage.remove(&key) {
                Some(_) => 0,
                None => 1,
            }
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "storage_has", |caller: Caller<'_, HostState>, key_ptr: i32, key_len: i32| -> i32 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let key = match read_string_from_guest(&memory, &caller, key_ptr, key_len, MAX_STORAGE_KEY_LEN) {
                Ok(k) => k,
                Err(_) => return -1,
            };

            if caller.data().storage.contains_key(&key) { 1 } else { 0 }
        }).map_err(|e| anyhow!("{}", e))?;

        Ok(())
    }

    fn register_context_functions(&self, linker: &mut Linker<HostState>, module: &str) -> Result<()> {
        linker.func_wrap(module, "get_caller", |mut caller: Caller<'_, HostState>| -> i32 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };
            let addr = caller.data().caller.clone();
            match write_to_guest(&memory, &mut caller, addr.as_bytes()) {
                Ok((ptr, _)) => ptr,
                Err(_) => -1,
            }
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "get_caller_len", |caller: Caller<'_, HostState>| -> i32 {
            caller.data().caller.len() as i32
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "get_block_height", |caller: Caller<'_, HostState>| -> i64 {
            caller.data().block_height as i64
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "get_timestamp", |caller: Caller<'_, HostState>| -> i64 {
            caller.data().timestamp as i64
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "get_contract_id", |mut caller: Caller<'_, HostState>| -> i32 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };
            let id = caller.data().contract_id.clone();
            match write_to_guest(&memory, &mut caller, id.as_bytes()) {
                Ok((ptr, _)) => ptr,
                Err(_) => -1,
            }
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "get_contract_id_len", |caller: Caller<'_, HostState>| -> i32 {
            caller.data().contract_id.len() as i32
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "get_input", |mut caller: Caller<'_, HostState>| -> i32 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };
            let input = caller.data().input_data.clone();
            let alloc_off = caller.data().alloc_offset;
            let mem_sz = memory.data_size(caller.as_context()) as u32;
            tracing::info!(
                "get_input: {} bytes, alloc_offset={}, mem_size={}, data={}",
                input.len(), alloc_off, mem_sz,
                String::from_utf8_lossy(&input[..input.len().min(200)])
            );
            match write_to_guest(&memory, &mut caller, &input) {
                Ok((ptr, len)) => {
                    tracing::debug!("get_input: wrote at ptr={}, len={}", ptr, len);
                    ptr
                },
                Err(e) => {
                    tracing::warn!("get_input: write_to_guest failed: {}", e);
                    -1
                },
            }
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "get_input_len", |caller: Caller<'_, HostState>| -> i32 {
            caller.data().input_data.len() as i32
        }).map_err(|e| anyhow!("{}", e))?;

        Ok(())
    }

    fn register_io_functions(&self, linker: &mut Linker<HostState>, module: &str) -> Result<()> {
        linker.func_wrap(module, "log", |mut caller: Caller<'_, HostState>, msg_ptr: i32, msg_len: i32| {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                _ => return,
            };

            let msg_bytes = match read_from_guest(&memory, &caller, msg_ptr, msg_len) {
                Ok(b) => b,
                Err(_) => return,
            };

            let msg = String::from_utf8_lossy(&msg_bytes[..msg_bytes.len().min(MAX_LOG_LEN)]).to_string();

            if caller.data().logs.len() < MAX_LOGS {
                caller.data_mut().gas_meter.charge("log", 1);
                caller.data_mut().logs.push(msg);
            }
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "emit_event", |mut caller: Caller<'_, HostState>, name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32| -> i32 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let event_name = match read_string_from_guest(&memory, &caller, name_ptr, name_len, MAX_STORAGE_KEY_LEN) {
                Ok(n) => n,
                Err(_) => return -1,
            };

            let data_bytes = match read_from_guest(&memory, &caller, data_ptr, data_len) {
                Ok(b) => b,
                Err(_) => return -1,
            };

            let event_data: HashMap<String, Value> = match serde_json::from_slice(&data_bytes) {
                Ok(d) => d,
                Err(_) => return -1,
            };

            if caller.data().events.len() >= MAX_EVENTS {
                return -1;
            }

            let index = caller.data().events.len();
            let contract_id = caller.data().contract_id.clone();

            caller.data_mut().gas_meter.charge("emit", 1);
            caller.data_mut().events.push(ContractEvent {
                contract_id,
                event_name,
                data: event_data,
                index,
            });
            0
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "set_return_data", |mut caller: Caller<'_, HostState>, data_ptr: i32, data_len: i32| {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                _ => return,
            };

            let data = match read_from_guest(&memory, &caller, data_ptr, data_len) {
                Ok(b) if b.len() <= MAX_RETURN_DATA => b,
                _ => return,
            };

            caller.data_mut().return_data = data;
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "set_error", |mut caller: Caller<'_, HostState>, msg_ptr: i32, msg_len: i32| {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                _ => return,
            };

            let msg_bytes = match read_from_guest(&memory, &caller, msg_ptr, msg_len) {
                Ok(b) => b,
                Err(_) => return,
            };

            let msg = String::from_utf8_lossy(&msg_bytes[..msg_bytes.len().min(MAX_LOG_LEN)]).to_string();
            caller.data_mut().error = Some(msg);
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "get_return_data_len", |caller: Caller<'_, HostState>| -> i32 {
            caller.data().return_data.len() as i32
        }).map_err(|e| anyhow!("{}", e))?;

        Ok(())
    }

    fn register_crypto_functions(&self, linker: &mut Linker<HostState>, module: &str) -> Result<()> {
        linker.func_wrap(module, "sha256", |mut caller: Caller<'_, HostState>, data_ptr: i32, data_len: i32| -> i32 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let data = match read_from_guest(&memory, &caller, data_ptr, data_len) {
                Ok(b) => b,
                Err(_) => return -1,
            };

            caller.data_mut().gas_meter.charge("hash", 1);

            use sha2::{Sha256, Digest};
            let hash = Sha256::digest(&data);

            match write_to_guest(&memory, &mut caller, hash.as_slice()) {
                Ok((ptr, _)) => ptr,
                Err(_) => -1,
            }
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "keccak256", |mut caller: Caller<'_, HostState>, data_ptr: i32, data_len: i32| -> i32 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let data = match read_from_guest(&memory, &caller, data_ptr, data_len) {
                Ok(b) => b,
                Err(_) => return -1,
            };

            caller.data_mut().gas_meter.charge("hash", 1);

            use sha2::{Sha256, Digest};
            let hash = Sha256::digest(&data);

            match write_to_guest(&memory, &mut caller, hash.as_slice()) {
                Ok((ptr, _)) => ptr,
                Err(_) => -1,
            }
        }).map_err(|e| anyhow!("{}", e))?;

        Ok(())
    }

    fn register_ledger_functions(&self, linker: &mut Linker<HostState>, module: &str) -> Result<()> {
        linker.func_wrap(module, "get_balance", |mut caller: Caller<'_, HostState>, addr_ptr: i32, addr_len: i32| -> i64 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let address = match read_string_from_guest(&memory, &caller, addr_ptr, addr_len, 64) {
                Ok(a) => a,
                Err(_) => return -1,
            };

            caller.data_mut().gas_meter.charge("balance_check", 1);

            match caller.data().accounts.get(&address) {
                Some(acct) => (acct.balance * 1_000_000.0) as i64,
                None => 0,
            }
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "get_staked", |mut caller: Caller<'_, HostState>, addr_ptr: i32, addr_len: i32| -> i64 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let address = match read_string_from_guest(&memory, &caller, addr_ptr, addr_len, 64) {
                Ok(a) => a,
                Err(_) => return -1,
            };

            caller.data_mut().gas_meter.charge("balance_check", 1);

            match caller.data().accounts.get(&address) {
                Some(acct) => (acct.staked * 1_000_000.0) as i64,
                None => 0,
            }
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "get_account_age", |mut caller: Caller<'_, HostState>, addr_ptr: i32, addr_len: i32| -> i64 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let address = match read_string_from_guest(&memory, &caller, addr_ptr, addr_len, 64) {
                Ok(a) => a,
                Err(_) => return -1,
            };

            caller.data_mut().gas_meter.charge("account_age_check", 1);

            let current_time = caller.data().timestamp;
            match caller.data().accounts.get(&address) {
                Some(acct) => (current_time.saturating_sub(acct.first_seen)) as i64,
                None => 0,
            }
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "get_nonce", |mut caller: Caller<'_, HostState>, addr_ptr: i32, addr_len: i32| -> i64 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let address = match read_string_from_guest(&memory, &caller, addr_ptr, addr_len, 64) {
                Ok(a) => a,
                Err(_) => return -1,
            };

            caller.data_mut().gas_meter.charge("balance_check", 1);

            match caller.data().accounts.get(&address) {
                Some(acct) => acct.nonce as i64,
                None => 0,
            }
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "transfer", |mut caller: Caller<'_, HostState>, from_ptr: i32, from_len: i32, to_ptr: i32, to_len: i32, amount_micro: i64| -> i32 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let from = match read_string_from_guest(&memory, &caller, from_ptr, from_len, 64) {
                Ok(a) => a,
                Err(_) => return -1,
            };

            let to = match read_string_from_guest(&memory, &caller, to_ptr, to_len, 64) {
                Ok(a) => a,
                Err(_) => return -1,
            };

            if amount_micro <= 0 {
                return -2;
            }

            let amount = amount_micro as f64 / 1_000_000.0;

            let caller_addr = caller.data().caller.clone();
            if from != caller_addr {
                return -3;
            }

            caller.data_mut().gas_meter.charge("transfer", 1);

            let from_balance = caller.data().accounts.get(&from).map(|a| a.balance).unwrap_or(0.0);
            if from_balance < amount {
                return -4;
            }

            if caller.data().transfer_ops.len() >= MAX_TRANSFERS {
                return -5;
            }

            let timestamp = caller.data().timestamp;
            let host = caller.data_mut();
            if let Some(acct) = host.accounts.get_mut(&from) {
                acct.balance -= amount;
            }
            let to_balance = host.accounts.get(&to).map(|a| a.balance).unwrap_or(0.0);
            if let Some(acct) = host.accounts.get_mut(&to) {
                acct.balance += amount;
            } else {
                host.accounts.insert(to.clone(), AccountSnapshot {
                    address: to.clone(),
                    balance: to_balance + amount,
                    nonce: 0,
                    first_seen: timestamp,
                    staked: 0.0,
                });
            }

            caller.data_mut().transfer_ops.push(TransferOp {
                from,
                to,
                amount,
            });

            0
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "emit_view_key", |mut caller: Caller<'_, HostState>, key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32| -> i32 {
            let memory = match get_memory(&caller) {
                Some(m) => m,
                None => return -1,
            };

            let key = match read_string_from_guest(&memory, &caller, key_ptr, key_len, MAX_STORAGE_KEY_LEN) {
                Ok(k) => k,
                Err(_) => return -1,
            };

            let val_bytes = match read_from_guest(&memory, &caller, val_ptr, val_len) {
                Ok(b) if b.len() <= MAX_STORAGE_VALUE_LEN => b,
                _ => return -1,
            };

            let value: Value = match serde_json::from_slice(&val_bytes) {
                Ok(v) => v,
                Err(_) => return -1,
            };

            if caller.data().view_key_emissions.len() >= MAX_VIEW_KEYS {
                return -1;
            }

            caller.data_mut().gas_meter.charge("emit", 1);
            caller.data_mut().view_key_emissions.push(ViewKeyEmission {
                key,
                value,
            });

            0
        }).map_err(|e| anyhow!("{}", e))?;

        linker.func_wrap(module, "get_gas_price", |caller: Caller<'_, HostState>| -> i64 {
            let host = caller.data();
            (host.gas_meter.gas_remaining()) as i64
        }).map_err(|e| anyhow!("{}", e))?;

        Ok(())
    }
}

impl Default for WasmEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn wrap_err(msg: String) -> WasmExecutionOutput {
    WasmExecutionOutput {
        result: ExecutionResult {
            success: false,
            state_diff: None,
            gas_used: 0,
            error: Some(msg),
            logs: Vec::new(),
            events: Vec::new(),
        },
        transfer_ops: Vec::new(),
        view_key_emissions: Vec::new(),
        return_data: Vec::new(),
    }
}

fn build_success_output(host: HostState, contract_id: &str, height: u64, consumed: u64) -> WasmExecutionOutput {
    let state_diff = compute_state_diff(
        contract_id,
        height,
        &host.original_storage,
        &host.storage,
    );
    let has_changes = !state_diff.changes.is_empty();
    WasmExecutionOutput {
        transfer_ops: host.transfer_ops.clone(),
        view_key_emissions: host.view_key_emissions.clone(),
        return_data: host.return_data.clone(),
        result: ExecutionResult {
            success: true,
            state_diff: if has_changes { Some(state_diff) } else { None },
            gas_used: consumed,
            error: None,
            logs: host.logs,
            events: host.events,
        },
    }
}

pub fn is_valid_wasm(data: &[u8]) -> bool {
    data.len() >= 8 && data[0..4] == [0x00, 0x61, 0x73, 0x6D]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_imports_wat() -> &'static str {
        r#"
            (import "rinku" "storage_read" (func $storage_read (param i32 i32) (result i32)))
            (import "rinku" "storage_read_len" (func $storage_read_len (param i32 i32) (result i32)))
            (import "rinku" "storage_write" (func $storage_write (param i32 i32 i32 i32) (result i32)))
            (import "rinku" "storage_delete" (func $storage_delete (param i32 i32) (result i32)))
            (import "rinku" "storage_has" (func $storage_has (param i32 i32) (result i32)))
            (import "rinku" "log" (func $log (param i32 i32)))
            (import "rinku" "emit_event" (func $emit_event (param i32 i32 i32 i32) (result i32)))
            (import "rinku" "get_caller" (func $get_caller (result i32)))
            (import "rinku" "get_caller_len" (func $get_caller_len (result i32)))
            (import "rinku" "get_block_height" (func $get_block_height (result i64)))
            (import "rinku" "get_timestamp" (func $get_timestamp (result i64)))
            (import "rinku" "get_contract_id" (func $get_contract_id (result i32)))
            (import "rinku" "get_contract_id_len" (func $get_contract_id_len (result i32)))
            (import "rinku" "get_input" (func $get_input (result i32)))
            (import "rinku" "get_input_len" (func $get_input_len (result i32)))
            (import "rinku" "sha256" (func $sha256 (param i32 i32) (result i32)))
            (import "rinku" "keccak256" (func $keccak256 (param i32 i32) (result i32)))
            (import "rinku" "set_return_data" (func $set_return_data (param i32 i32)))
            (import "rinku" "set_error" (func $set_error (param i32 i32)))
            (import "rinku" "get_return_data_len" (func $get_return_data_len (result i32)))
            (import "rinku" "get_balance" (func $get_balance (param i32 i32) (result i64)))
            (import "rinku" "get_staked" (func $get_staked (param i32 i32) (result i64)))
            (import "rinku" "get_account_age" (func $get_account_age (param i32 i32) (result i64)))
            (import "rinku" "get_nonce" (func $get_nonce (param i32 i32) (result i64)))
            (import "rinku" "transfer" (func $transfer (param i32 i32 i32 i32 i64) (result i32)))
            (import "rinku" "emit_view_key" (func $emit_view_key (param i32 i32 i32 i32) (result i32)))
            (import "rinku" "get_gas_price" (func $get_gas_price (result i64)))
        "#
    }

    fn make_simple_wasm() -> Vec<u8> {
        let wat = format!(r#"
            (module
                {}
                (memory (export "memory") 1)
                (data (i32.const 0) "hello from wasm")

                (func (export "hello") (result i32)
                    (call $log (i32.const 0) (i32.const 15))
                    (i32.const 0)
                )
            )
        "#, all_imports_wat());
        wat::parse_str(&wat).expect("Failed to parse WAT")
    }

    fn make_counter_wasm() -> Vec<u8> {
        let wat = format!(r#"
            (module
                {}
                (memory (export "memory") 1)
                (data (i32.const 0) "count")
                (data (i32.const 5) "42")
                (data (i32.const 7) "counter initialized")

                (func (export "init") (result i32)
                    (call $storage_write
                        (i32.const 0) (i32.const 5)
                        (i32.const 5) (i32.const 2)
                    )
                    drop
                    (call $log (i32.const 7) (i32.const 19))
                    (i32.const 0)
                )
            )
        "#, all_imports_wat());
        wat::parse_str(&wat).expect("Failed to parse WAT")
    }

    #[test]
    fn test_is_valid_wasm() {
        let wasm = make_simple_wasm();
        assert!(is_valid_wasm(&wasm));
        assert!(!is_valid_wasm(b"not wasm"));
        assert!(!is_valid_wasm(&[]));
    }

    #[test]
    fn test_wasm_engine_validate() {
        let engine = WasmEngine::new();
        let wasm = make_simple_wasm();
        assert!(engine.validate_wasm(&wasm).is_ok());
        assert!(engine.validate_wasm(b"invalid").is_err());
    }

    #[test]
    fn test_wasm_engine_execute_hello() {
        let engine = WasmEngine::new();
        let wasm = make_simple_wasm();
        let input = HashMap::new();
        let state = HashMap::new();

        let output = engine.execute("test_contract", &wasm, "hello", &input, &state, 1, Some(1_000_000), "caller_addr", 100);

        assert!(output.result.success);
        assert_eq!(output.result.logs, vec!["hello from wasm"]);
        assert!(output.result.gas_used > 0);
    }

    #[test]
    fn test_wasm_engine_execute_init_counter() {
        let engine = WasmEngine::new();
        let wasm = make_counter_wasm();
        let input = HashMap::new();
        let state = HashMap::new();

        let output = engine.execute("counter_contract", &wasm, "init", &input, &state, 1, Some(1_000_000), "caller_addr", 100);

        assert!(output.result.success);
        assert!(output.result.state_diff.is_some());
        let diff = output.result.state_diff.unwrap();
        assert!(!diff.changes.is_empty());
        assert_eq!(output.result.logs, vec!["counter initialized"]);
    }

    #[test]
    fn test_wasm_engine_missing_entrypoint() {
        let engine = WasmEngine::new();
        let wasm = make_simple_wasm();
        let input = HashMap::new();
        let state = HashMap::new();

        let output = engine.execute("test_contract", &wasm, "nonexistent", &input, &state, 1, Some(1_000_000), "caller_addr", 100);

        assert!(!output.result.success);
        assert!(output.result.error.unwrap().contains("not found"));
    }

    #[test]
    fn test_wasm_engine_out_of_fuel() {
        let engine = WasmEngine::new();
        let wasm = make_simple_wasm();
        let input = HashMap::new();
        let state = HashMap::new();

        let output = engine.execute("test_contract", &wasm, "hello", &input, &state, 1, Some(1), "caller_addr", 100);

        assert!(!output.result.success);
    }

    #[test]
    fn test_wasm_context_functions() {
        let engine = WasmEngine::new();
        let wasm = make_simple_wasm();
        let input = HashMap::new();
        let state = HashMap::new();

        let output = engine.execute("my_contract", &wasm, "hello", &input, &state, 42, Some(1_000_000), "alice", 1234567890);

        assert!(output.result.success);
    }

    #[test]
    fn test_get_balance_host_function() {
        let engine = WasmEngine::new();

        let wat = format!(r#"
            (module
                {}
                (memory (export "memory") 1)
                (data (i32.const 0) "alice_address_here")

                (func (export "check_balance") (result i32)
                    (i64.gt_s
                        (call $get_balance (i32.const 0) (i32.const 18))
                        (i64.const 0)
                    )
                    (if (result i32) (then (i32.const 0)) (else (i32.const 1)))
                )
            )
        "#, all_imports_wat());
        let wasm = wat::parse_str(&wat).expect("Failed to parse WAT");

        let mut accounts = HashMap::new();
        accounts.insert("alice_address_here".to_string(), AccountSnapshot {
            address: "alice_address_here".to_string(),
            balance: 100.0,
            nonce: 5,
            first_seen: 1000,
            staked: 50.0,
        });

        let ctx = ExecutionContext {
            accounts,
            gas_price: 1.0,
            max_memory_pages: MAX_MEMORY_PAGES,
        };

        let output = engine.execute_with_context(
            "test", &wasm, "check_balance", &HashMap::new(), &HashMap::new(),
            1, Some(1_000_000), "caller", 2000, ctx,
        );

        assert!(output.result.success, "Error: {:?}", output.result.error);
    }

    #[test]
    fn test_transfer_host_function() {
        let engine = WasmEngine::new();

        let wat = format!(r#"
            (module
                {}
                (memory (export "memory") 1)
                (data (i32.const 0) "alice")
                (data (i32.const 5) "bob00")

                (func (export "do_transfer") (result i32)
                    (call $transfer
                        (i32.const 0) (i32.const 5)
                        (i32.const 5) (i32.const 5)
                        (i64.const 10000000)
                    )
                )
            )
        "#, all_imports_wat());
        let wasm = wat::parse_str(&wat).expect("Failed to parse WAT");

        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), AccountSnapshot {
            address: "alice".to_string(),
            balance: 100.0,
            nonce: 0,
            first_seen: 1000,
            staked: 0.0,
        });
        accounts.insert("bob00".to_string(), AccountSnapshot {
            address: "bob00".to_string(),
            balance: 50.0,
            nonce: 0,
            first_seen: 1000,
            staked: 0.0,
        });

        let ctx = ExecutionContext {
            accounts,
            gas_price: 1.0,
            max_memory_pages: MAX_MEMORY_PAGES,
        };

        let output = engine.execute_with_context(
            "test", &wasm, "do_transfer", &HashMap::new(), &HashMap::new(),
            1, Some(1_000_000), "alice", 2000, ctx,
        );

        assert!(output.result.success, "Error: {:?}", output.result.error);
        assert_eq!(output.transfer_ops.len(), 1);
        assert_eq!(output.transfer_ops[0].from, "alice");
        assert_eq!(output.transfer_ops[0].to, "bob00");
        assert!((output.transfer_ops[0].amount - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_transfer_insufficient_balance() {
        let engine = WasmEngine::new();

        let wat = format!(r#"
            (module
                {}
                (memory (export "memory") 1)
                (data (i32.const 0) "alice")
                (data (i32.const 5) "bob00")

                (func (export "do_transfer") (result i32)
                    (call $transfer
                        (i32.const 0) (i32.const 5)
                        (i32.const 5) (i32.const 5)
                        (i64.const 999000000)
                    )
                )
            )
        "#, all_imports_wat());
        let wasm = wat::parse_str(&wat).expect("Failed to parse WAT");

        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), AccountSnapshot {
            address: "alice".to_string(),
            balance: 10.0,
            nonce: 0,
            first_seen: 1000,
            staked: 0.0,
        });

        let ctx = ExecutionContext {
            accounts,
            gas_price: 1.0,
            max_memory_pages: MAX_MEMORY_PAGES,
        };

        let output = engine.execute_with_context(
            "test", &wasm, "do_transfer", &HashMap::new(), &HashMap::new(),
            1, Some(1_000_000), "alice", 2000, ctx,
        );

        assert!(!output.result.success);
    }

    #[test]
    fn test_emit_view_key() {
        let engine = WasmEngine::new();

        let wat = format!(r#"
            (module
                {}
                (memory (export "memory") 1)
                (data (i32.const 0) "balance")
                (data (i32.const 7) "42")

                (func (export "emit_vk") (result i32)
                    (call $emit_view_key
                        (i32.const 0) (i32.const 7)
                        (i32.const 7) (i32.const 2)
                    )
                )
            )
        "#, all_imports_wat());
        let wasm = wat::parse_str(&wat).expect("Failed to parse WAT");

        let output = engine.execute(
            "test", &wasm, "emit_vk", &HashMap::new(), &HashMap::new(),
            1, Some(1_000_000), "caller", 100,
        );

        assert!(output.result.success, "Error: {:?}", output.result.error);
        assert_eq!(output.view_key_emissions.len(), 1);
        assert_eq!(output.view_key_emissions[0].key, "balance");
    }

    #[test]
    fn test_memory_limit_enforcement() {
        let engine = WasmEngine::new();

        let wat = format!(r#"
            (module
                {}
                (memory (export "memory") 1 256)
                (func (export "hello") (result i32)
                    (i32.const 0)
                )
            )
        "#, all_imports_wat());
        let wasm = wat::parse_str(&wat).expect("Failed to parse WAT");

        let ctx = ExecutionContext {
            accounts: HashMap::new(),
            gas_price: 1.0,
            max_memory_pages: 2,
        };

        let output = engine.execute_with_context(
            "test", &wasm, "hello", &HashMap::new(), &HashMap::new(),
            1, Some(1_000_000), "caller", 100, ctx,
        );

        assert!(output.result.success);
    }

    #[test]
    fn test_validate_rejects_foreign_imports() {
        let engine = WasmEngine::new();

        let wat = r#"
            (module
                (import "evil" "steal" (func (result i32)))
                (memory (export "memory") 1)
                (func (export "hello") (result i32)
                    (i32.const 0)
                )
            )
        "#;
        let wasm = wat::parse_str(wat).expect("Failed to parse WAT");

        let result = engine.validate_wasm(&wasm);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown namespace"));
    }

    #[test]
    fn test_storage_has() {
        let engine = WasmEngine::new();

        let wat = format!(r#"
            (module
                {}
                (memory (export "memory") 1)
                (data (i32.const 0) "mykey")
                (data (i32.const 5) "nokey")

                (func (export "check") (result i32)
                    ;; mykey exists => storage_has returns 1, test passes (return 0)
                    ;; nokey missing => storage_has returns 0, test fails (return 1)
                    (if (result i32) (i32.eq (call $storage_has (i32.const 0) (i32.const 5)) (i32.const 1))
                        (then
                            (if (result i32) (i32.eq (call $storage_has (i32.const 5) (i32.const 5)) (i32.const 0))
                                (then (i32.const 0))
                                (else (i32.const 2))
                            )
                        )
                        (else (i32.const 1))
                    )
                )
            )
        "#, all_imports_wat());
        let wasm = wat::parse_str(&wat).expect("Failed to parse WAT");

        let mut state = HashMap::new();
        state.insert("mykey".to_string(), Value::Number(42.into()));

        let output = engine.execute(
            "test", &wasm, "check", &HashMap::new(), &state,
            1, Some(1_000_000), "caller", 100,
        );

        assert!(output.result.success, "Error: {:?}", output.result.error);
    }

    #[test]
    fn test_execute_full_transfer_and_view_key_pipeline() {
        let engine = WasmEngine::new();

        let wat = format!(r#"
            (module
                {}
                (memory (export "memory") 1)
                ;; "alice_addr" at offset 0 (10 bytes)
                (data (i32.const 0) "alice_addr")
                ;; "bob_address" at offset 16 (11 bytes)
                (data (i32.const 16) "bob_address")
                ;; "balance" at offset 32 (7 bytes)
                (data (i32.const 32) "balance")
                ;; "500" at offset 48 (3 bytes)
                (data (i32.const 48) "500")

                (func (export "run") (result i32)
                    ;; Transfer 5.0 RKU (5_000_000 micro) from alice to bob
                    (call $transfer
                        (i32.const 0) (i32.const 10)   ;; from: "alice_addr"
                        (i32.const 16) (i32.const 11)  ;; to: "bob_address"
                        (i64.const 5000000)             ;; 5.0 RKU in micro
                    )
                    ;; If transfer failed, return the error code
                    (if (result i32) (i32.ne (i32.const 0))
                        (then
                            (i32.const 100)
                        )
                        (else
                            ;; Emit view key: "balance" -> "500"
                            (call $emit_view_key
                                (i32.const 32) (i32.const 7)
                                (i32.const 48) (i32.const 3)
                            )
                            drop
                            (i32.const 0)
                        )
                    )
                )
            )
        "#, all_imports_wat());
        let wasm = wat::parse_str(&wat).expect("Failed to parse WAT");

        let mut accounts = HashMap::new();
        accounts.insert("alice_addr".to_string(), AccountSnapshot {
            address: "alice_addr".to_string(),
            balance: 100.0,
            nonce: 0,
            first_seen: 1000,
            staked: 0.0,
        });
        accounts.insert("bob_address".to_string(), AccountSnapshot {
            address: "bob_address".to_string(),
            balance: 10.0,
            nonce: 0,
            first_seen: 1000,
            staked: 0.0,
        });

        let ctx = ExecutionContext {
            accounts,
            gas_price: 1.0,
            max_memory_pages: MAX_MEMORY_PAGES,
        };

        let output = engine.execute_with_context(
            "test_contract", &wasm, "run", &HashMap::new(), &HashMap::new(),
            1, Some(1_000_000), "alice_addr", 2000, ctx,
        );

        assert!(output.result.success, "Execution failed: {:?}", output.result.error);

        assert_eq!(output.transfer_ops.len(), 1, "Expected 1 transfer op");
        let transfer = &output.transfer_ops[0];
        assert_eq!(transfer.from, "alice_addr");
        assert_eq!(transfer.to, "bob_address");
        assert!((transfer.amount - 5.0).abs() < 0.001, "Expected ~5.0 RKU, got {}", transfer.amount);

        assert_eq!(output.view_key_emissions.len(), 1, "Expected 1 view key emission");
        let vk = &output.view_key_emissions[0];
        assert_eq!(vk.key, "balance");
    }

    #[test]
    fn test_execute_full_multiple_transfers_and_balance_tracking() {
        let engine = WasmEngine::new();

        let wat = format!(r#"
            (module
                {}
                (memory (export "memory") 1)
                (data (i32.const 0) "alice_addr")
                (data (i32.const 16) "bob_address")
                (data (i32.const 32) "carol_addr_")

                (func (export "multi_transfer") (result i32)
                    ;; Transfer 2.0 RKU from alice to bob
                    (call $transfer
                        (i32.const 0) (i32.const 10)
                        (i32.const 16) (i32.const 11)
                        (i64.const 2000000)
                    )
                    (if (result i32) (i32.ne (i32.const 0))
                        (then (i32.const 1))
                        (else
                            ;; Transfer 3.0 RKU from alice to carol
                            (call $transfer
                                (i32.const 0) (i32.const 10)
                                (i32.const 32) (i32.const 11)
                                (i64.const 3000000)
                            )
                            (if (result i32) (i32.ne (i32.const 0))
                                (then (i32.const 2))
                                (else (i32.const 0))
                            )
                        )
                    )
                )
            )
        "#, all_imports_wat());
        let wasm = wat::parse_str(&wat).expect("Failed to parse WAT");

        let mut accounts = HashMap::new();
        accounts.insert("alice_addr".to_string(), AccountSnapshot {
            address: "alice_addr".to_string(),
            balance: 50.0,
            nonce: 0,
            first_seen: 1000,
            staked: 0.0,
        });

        let ctx = ExecutionContext {
            accounts,
            gas_price: 1.0,
            max_memory_pages: MAX_MEMORY_PAGES,
        };

        let output = engine.execute_with_context(
            "test_contract", &wasm, "multi_transfer", &HashMap::new(), &HashMap::new(),
            1, Some(1_000_000), "alice_addr", 2000, ctx,
        );

        assert!(output.result.success, "Execution failed: {:?}", output.result.error);
        assert_eq!(output.transfer_ops.len(), 2, "Expected 2 transfer ops");

        assert_eq!(output.transfer_ops[0].from, "alice_addr");
        assert_eq!(output.transfer_ops[0].to, "bob_address");
        assert!((output.transfer_ops[0].amount - 2.0).abs() < 0.001);

        assert_eq!(output.transfer_ops[1].from, "alice_addr");
        assert_eq!(output.transfer_ops[1].to, "carol_addr_");
        assert!((output.transfer_ops[1].amount - 3.0).abs() < 0.001);
    }

    #[test]
    fn test_contract_storage_integration_round_trip() {
        use crate::contract_storage::ContractStorageManager;

        let mut mgr = ContractStorageManager::new();

        mgr.write_key("token_contract", "total_supply", &Value::from(1000000), None).unwrap();
        mgr.write_key("token_contract", "balance:alice", &Value::from(500), None).unwrap();
        mgr.write_key("token_contract", "balance:bob", &Value::from(300), None).unwrap();

        let pre_root = mgr.root();

        let proof_alice = mgr.prove_key("token_contract", "balance:alice", None).unwrap();
        assert!(proof_alice.verify());
        assert_eq!(proof_alice.root, pre_root);

        let changes = vec![
            ("balance:alice".to_string(), Some(Value::from(450))),
            ("balance:bob".to_string(), Some(Value::from(350))),
        ];
        let new_root = mgr.apply_state_diff("token_contract", &changes, None).unwrap();
        assert_ne!(pre_root, new_root);

        assert_eq!(
            mgr.read_key("token_contract", "balance:alice", None).unwrap(),
            Some(Value::from(450))
        );
        assert_eq!(
            mgr.read_key("token_contract", "balance:bob", None).unwrap(),
            Some(Value::from(350))
        );

        let proof_alice_post = mgr.prove_key("token_contract", "balance:alice", None).unwrap();
        assert!(proof_alice_post.verify());
        assert_eq!(proof_alice_post.root, new_root);

        assert_eq!(
            mgr.read_key("token_contract", "total_supply", None).unwrap(),
            Some(Value::from(1000000))
        );
    }
}
