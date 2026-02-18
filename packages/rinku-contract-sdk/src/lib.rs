pub mod host;
pub mod types;
pub mod storage;
pub mod ledger;

pub use types::*;

pub use host::{
    log, get_block_height, get_timestamp,
    get_caller, get_contract_id, get_input_json,
    set_return_data, set_error_msg, get_gas_remaining,
};

pub use storage::{
    storage_get, storage_get_or_default, storage_set,
    storage_delete, storage_has, storage_get_u64,
    storage_get_i64, storage_get_string, storage_get_bool,
    storage_increment,
};

pub use ledger::{
    get_balance, get_balance_micro, get_staked,
    transfer, transfer_from, emit_view_key, emit_event,
    set_return_json, sha256, require, require_caller,
};

pub use ledger::get_account_age;
pub use ledger::get_nonce;

#[macro_export]
macro_rules! entrypoint {
    ($func:ident) => {
        #[no_mangle]
        pub extern "C" fn $func() -> i32 {
            match $crate::_run_entrypoint(|| {
                let input = $crate::get_input_json();
                $func(input)
            }) {
                Ok(()) => 0,
                Err(code) => code,
            }
        }
    };
}

#[macro_export]
macro_rules! contract_init {
    ($func:ident) => {
        #[no_mangle]
        pub extern "C" fn init() -> i32 {
            match $crate::_run_entrypoint(|| {
                let input = $crate::get_input_json();
                $func(input)
            }) {
                Ok(()) => 0,
                Err(code) => code,
            }
        }
    };
}

#[macro_export]
macro_rules! contract_call {
    ($name:ident, $func:ident) => {
        #[no_mangle]
        pub extern "C" fn $name() -> i32 {
            match $crate::_run_entrypoint(|| {
                let input = $crate::get_input_json();
                $func(input)
            }) {
                Ok(()) => 0,
                Err(code) => code,
            }
        }
    };
}

pub fn _run_entrypoint<F>(f: F) -> Result<(), i32>
where
    F: FnOnce() -> Result<(), ContractError>,
{
    match f() {
        Ok(()) => Ok(()),
        Err(e) => {
            host::set_error_msg(&e.message);
            Err(e.code)
        }
    }
}
