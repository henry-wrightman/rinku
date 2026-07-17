use crate::host;
use crate::types::ContractError;
use serde::{de::DeserializeOwned, Serialize};

pub fn storage_get<T: DeserializeOwned>(key: &str) -> Result<Option<T>, ContractError> {
    let len = host::raw_storage_read_len(key.as_bytes());
    if len <= 0 {
        return Ok(None);
    }

    let ptr = host::raw_storage_read(key.as_bytes());
    if ptr <= 0 {
        return Ok(None);
    }

    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize).to_vec() };

    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| ContractError::storage_error(format!("Deserialize failed: {}", e)))
}

pub fn storage_get_or_default<T: DeserializeOwned + Default>(key: &str) -> T {
    storage_get::<T>(key).ok().flatten().unwrap_or_default()
}

pub fn storage_set<T: Serialize>(key: &str, value: &T) -> Result<(), ContractError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| ContractError::storage_error(format!("Serialize failed: {}", e)))?;

    let result = host::raw_storage_write(key.as_bytes(), &bytes);
    if result != 0 {
        return Err(ContractError::storage_error("Storage write failed"));
    }
    Ok(())
}

pub fn storage_delete(key: &str) {
    host::raw_storage_delete(key.as_bytes());
}

pub fn storage_has(key: &str) -> bool {
    host::raw_storage_has(key.as_bytes())
}

pub fn storage_get_u64(key: &str) -> u64 {
    storage_get::<u64>(key).ok().flatten().unwrap_or(0)
}

pub fn storage_get_i64(key: &str) -> i64 {
    storage_get::<i64>(key).ok().flatten().unwrap_or(0)
}

pub fn storage_get_string(key: &str) -> Option<String> {
    storage_get::<String>(key).ok().flatten()
}

pub fn storage_get_bool(key: &str) -> bool {
    storage_get::<bool>(key).ok().flatten().unwrap_or(false)
}

pub fn storage_increment(key: &str, delta: i64) -> Result<i64, ContractError> {
    let current = storage_get_i64(key);
    let new_val = current
        .checked_add(delta)
        .ok_or(ContractError::overflow())?;
    storage_set(key, &new_val)?;
    Ok(new_val)
}
