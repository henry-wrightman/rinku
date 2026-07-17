#[link(wasm_import_module = "rinku")]
extern "C" {
    #[link_name = "storage_read"]
    fn _storage_read(key_ptr: i32, key_len: i32) -> i32;

    #[link_name = "storage_read_len"]
    fn _storage_read_len(key_ptr: i32, key_len: i32) -> i32;

    #[link_name = "storage_write"]
    fn _storage_write(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32) -> i32;

    #[link_name = "storage_delete"]
    fn _storage_delete(key_ptr: i32, key_len: i32) -> i32;

    #[link_name = "storage_has"]
    fn _storage_has(key_ptr: i32, key_len: i32) -> i32;

    #[link_name = "log"]
    fn _log(msg_ptr: i32, msg_len: i32);

    #[link_name = "emit_event"]
    fn _emit_event(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32;

    #[link_name = "get_caller"]
    fn _get_caller() -> i32;

    #[link_name = "get_caller_len"]
    fn _get_caller_len() -> i32;

    #[link_name = "get_block_height"]
    fn _get_block_height() -> i64;

    #[link_name = "get_timestamp"]
    fn _get_timestamp() -> i64;

    #[link_name = "get_contract_id"]
    fn _get_contract_id() -> i32;

    #[link_name = "get_contract_id_len"]
    fn _get_contract_id_len() -> i32;

    #[link_name = "get_input"]
    fn _get_input() -> i32;

    #[link_name = "get_input_len"]
    fn _get_input_len() -> i32;

    #[link_name = "sha256"]
    fn _sha256(data_ptr: i32, data_len: i32) -> i32;

    #[link_name = "keccak256"]
    fn _keccak256(data_ptr: i32, data_len: i32) -> i32;

    #[link_name = "set_return_data"]
    fn _set_return_data(data_ptr: i32, data_len: i32);

    #[link_name = "set_error"]
    fn _set_error(msg_ptr: i32, msg_len: i32);

    #[link_name = "get_return_data_len"]
    fn _get_return_data_len() -> i32;

    #[link_name = "get_balance"]
    fn _get_balance(addr_ptr: i32, addr_len: i32) -> i64;

    #[link_name = "get_staked"]
    fn _get_staked(addr_ptr: i32, addr_len: i32) -> i64;

    #[link_name = "get_account_age"]
    fn _get_account_age(addr_ptr: i32, addr_len: i32) -> i64;

    #[link_name = "get_nonce"]
    fn _get_nonce(addr_ptr: i32, addr_len: i32) -> i64;

    #[link_name = "transfer"]
    fn _transfer(from_ptr: i32, from_len: i32, to_ptr: i32, to_len: i32, amount_micro: i64) -> i32;

    #[link_name = "emit_view_key"]
    fn _emit_view_key(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32) -> i32;

    #[link_name = "get_gas_price"]
    fn _get_gas_price() -> i64;
}

pub fn raw_storage_read(key: &[u8]) -> i32 {
    unsafe { _storage_read(key.as_ptr() as i32, key.len() as i32) }
}

pub fn raw_storage_read_len(key: &[u8]) -> i32 {
    unsafe { _storage_read_len(key.as_ptr() as i32, key.len() as i32) }
}

pub fn raw_storage_write(key: &[u8], value: &[u8]) -> i32 {
    unsafe {
        _storage_write(
            key.as_ptr() as i32,
            key.len() as i32,
            value.as_ptr() as i32,
            value.len() as i32,
        )
    }
}

pub fn raw_storage_delete(key: &[u8]) -> i32 {
    unsafe { _storage_delete(key.as_ptr() as i32, key.len() as i32) }
}

pub fn raw_storage_has(key: &[u8]) -> bool {
    unsafe { _storage_has(key.as_ptr() as i32, key.len() as i32) == 1 }
}

pub fn log(msg: &str) {
    unsafe { _log(msg.as_ptr() as i32, msg.len() as i32) }
}

pub fn emit_event_raw(name: &str, data: &[u8]) -> i32 {
    unsafe {
        _emit_event(
            name.as_ptr() as i32,
            name.len() as i32,
            data.as_ptr() as i32,
            data.len() as i32,
        )
    }
}

pub fn get_caller_raw() -> (i32, i32) {
    unsafe {
        let len = _get_caller_len();
        let ptr = _get_caller();
        (ptr, len)
    }
}

pub fn get_block_height() -> u64 {
    unsafe { _get_block_height() as u64 }
}

pub fn get_timestamp() -> u64 {
    unsafe { _get_timestamp() as u64 }
}

pub fn get_contract_id_raw() -> (i32, i32) {
    unsafe {
        let len = _get_contract_id_len();
        let ptr = _get_contract_id();
        (ptr, len)
    }
}

pub fn get_input_raw() -> (i32, i32) {
    unsafe {
        let len = _get_input_len();
        let ptr = _get_input();
        (ptr, len)
    }
}

pub fn sha256_raw(data: &[u8]) -> i32 {
    unsafe { _sha256(data.as_ptr() as i32, data.len() as i32) }
}

pub fn keccak256_raw(data: &[u8]) -> i32 {
    unsafe { _keccak256(data.as_ptr() as i32, data.len() as i32) }
}

pub fn set_return_data(data: &[u8]) {
    unsafe { _set_return_data(data.as_ptr() as i32, data.len() as i32) }
}

pub fn set_error_msg(msg: &str) {
    unsafe { _set_error(msg.as_ptr() as i32, msg.len() as i32) }
}

pub fn get_balance_micro(address: &str) -> i64 {
    unsafe { _get_balance(address.as_ptr() as i32, address.len() as i32) }
}

pub fn get_staked_micro(address: &str) -> i64 {
    unsafe { _get_staked(address.as_ptr() as i32, address.len() as i32) }
}

pub fn get_account_age(address: &str) -> u64 {
    unsafe { _get_account_age(address.as_ptr() as i32, address.len() as i32) as u64 }
}

pub fn get_nonce(address: &str) -> u64 {
    unsafe { _get_nonce(address.as_ptr() as i32, address.len() as i32) as u64 }
}

pub fn raw_transfer(from: &str, to: &str, amount_micro: i64) -> i32 {
    unsafe {
        _transfer(
            from.as_ptr() as i32,
            from.len() as i32,
            to.as_ptr() as i32,
            to.len() as i32,
            amount_micro,
        )
    }
}

pub fn raw_emit_view_key(key: &str, value: &[u8]) -> i32 {
    unsafe {
        _emit_view_key(
            key.as_ptr() as i32,
            key.len() as i32,
            value.as_ptr() as i32,
            value.len() as i32,
        )
    }
}

pub fn get_gas_remaining() -> u64 {
    unsafe { _get_gas_price() as u64 }
}

unsafe fn read_guest_bytes(ptr: i32, len: i32) -> Vec<u8> {
    let slice = core::slice::from_raw_parts(ptr as *const u8, len as usize);
    slice.to_vec()
}

pub fn get_caller() -> String {
    let (ptr, len) = get_caller_raw();
    if ptr < 0 {
        return String::new();
    }
    unsafe {
        let bytes = read_guest_bytes(ptr, len);
        String::from_utf8_lossy(&bytes).to_string()
    }
}

pub fn get_contract_id() -> String {
    let (ptr, len) = get_contract_id_raw();
    if ptr < 0 {
        return String::new();
    }
    unsafe {
        let bytes = read_guest_bytes(ptr, len);
        String::from_utf8_lossy(&bytes).to_string()
    }
}

pub fn get_input_json() -> serde_json::Value {
    let (ptr, len) = get_input_raw();
    if ptr < 0 || len <= 0 {
        return serde_json::Value::Object(Default::default());
    }
    unsafe {
        let bytes = read_guest_bytes(ptr, len);
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Object(Default::default()))
    }
}
