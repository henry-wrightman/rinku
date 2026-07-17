pub mod host;
pub mod ledger;
pub mod storage;
pub mod types;

pub use types::*;

pub use host::{
    get_block_height, get_caller, get_contract_id, get_gas_remaining, get_input_json,
    get_timestamp, log, set_error_msg, set_return_data,
};

pub use storage::{
    storage_delete, storage_get, storage_get_bool, storage_get_i64, storage_get_or_default,
    storage_get_string, storage_get_u64, storage_has, storage_increment, storage_set,
};

pub use ledger::{
    emit_event, emit_view_key, get_balance, get_balance_micro, get_staked, require, require_caller,
    set_return_json, sha256, transfer, transfer_from,
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
