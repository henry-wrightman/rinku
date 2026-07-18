use crate::host;
use crate::types::{ContractError, ContractResult, EventData};

pub fn get_balance(address: &str) -> f64 {
    let micro = host::get_balance_micro(address);
    if micro < 0 {
        return 0.0;
    }
    micro as f64 / 1_000_000.0
}

pub fn get_balance_micro(address: &str) -> i64 {
    host::get_balance_micro(address)
}

pub fn get_staked(address: &str) -> f64 {
    let micro = host::get_staked_micro(address);
    if micro < 0 {
        return 0.0;
    }
    micro as f64 / 1_000_000.0
}

pub fn get_account_age(address: &str) -> u64 {
    host::get_account_age(address)
}

pub fn get_nonce(address: &str) -> u64 {
    host::get_nonce(address)
}

pub fn transfer(to: &str, amount: f64) -> ContractResult {
    let caller = host::get_caller();
    transfer_from(&caller, to, amount)
}

pub fn transfer_from(from: &str, to: &str, amount: f64) -> ContractResult {
    let amount_micro = (amount * 1_000_000.0) as i64;
    if amount_micro <= 0 {
        return Err(ContractError::invalid_input(
            "Transfer amount must be positive",
        ));
    }

    let result = host::raw_transfer(from, to, amount_micro);
    match result {
        0 => Ok(()),
        -2 => Err(ContractError::invalid_input("Invalid amount")),
        -3 => Err(ContractError::unauthorized()),
        -4 => Err(ContractError::insufficient_balance()),
        -5 => Err(ContractError::internal("Too many transfers in one call")),
        _ => Err(ContractError::internal(format!(
            "Transfer failed: {}",
            result
        ))),
    }
}

pub fn emit_view_key(key: &str, value: &serde_json::Value) -> ContractResult {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| ContractError::internal(format!("Serialize view key: {}", e)))?;

    let result = host::raw_emit_view_key(key, &bytes);
    if result != 0 {
        return Err(ContractError::internal("emit_view_key failed"));
    }
    Ok(())
}

pub fn emit_event(name: &str, data: EventData) -> ContractResult {
    let bytes = data.to_bytes();
    let result = host::emit_event_raw(name, &bytes);
    if result != 0 {
        return Err(ContractError::internal("emit_event failed"));
    }
    Ok(())
}

pub fn set_return_json(value: &serde_json::Value) {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    host::set_return_data(&bytes);
}

pub fn sha256(data: &[u8]) -> Vec<u8> {
    let ptr = host::sha256_raw(data);
    if ptr < 0 {
        return Vec::new();
    }
    unsafe { core::slice::from_raw_parts(ptr as *const u8, 32).to_vec() }
}

pub fn require(condition: bool, msg: &str) -> ContractResult {
    if !condition {
        return Err(ContractError::invalid_input(msg));
    }
    Ok(())
}

pub fn require_caller(expected: &str) -> ContractResult {
    let caller = host::get_caller();
    if caller != expected {
        return Err(ContractError::unauthorized());
    }
    Ok(())
}
