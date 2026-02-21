# Rinku WASM Host ABI Reference

This document describes the host functions available to Rinku smart contracts running in the WASM runtime. All host functions are imported from the `rinku` namespace.

## Module Validation

- Only imports from the `rinku` namespace are allowed. Contracts importing from any other namespace will be rejected at load time.
- Maximum memory: 16 pages (1 MB). Contracts requesting more will fail validation.
- Contracts must export a `memory` of at least 1 page.

## Host Function Categories

### 1. Storage Functions

Contract-local key-value storage. Keys and values are serialized as JSON bytes.

| Function | Signature | Returns | Description |
|---|---|---|---|
| `storage_read` | `(key_ptr: i32, key_len: i32) -> i32` | Pointer to value in guest memory, or `-1` if not found, `-2` if key too long | Read a value from contract storage |
| `storage_read_len` | `(key_ptr: i32, key_len: i32) -> i32` | Length of value in bytes, or `-1` if not found | Get the byte length of a stored value without reading it |
| `storage_write` | `(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32) -> i32` | `0` on success, `-1` if key too long, `-2` if value too long | Write a value to contract storage |
| `storage_delete` | `(key_ptr: i32, key_len: i32) -> i32` | `0` on success | Delete a key from contract storage |
| `storage_has` | `(key_ptr: i32, key_len: i32) -> i32` | `1` if key exists, `0` if not | Check whether a key exists in storage |

**Limits:**
- Maximum key length: 256 bytes
- Maximum value length: 8,192 bytes

### 2. Context Functions

Read-only access to execution context (caller, block info, contract identity).

| Function | Signature | Returns | Description |
|---|---|---|---|
| `get_caller` | `() -> i32` | Pointer to caller address in guest memory | Get the address of the transaction sender |
| `get_caller_len` | `() -> i32` | Length of caller address string | Get the byte length of the caller address |
| `get_block_height` | `() -> i64` | Current block/checkpoint height | Get the current block height |
| `get_timestamp` | `() -> i64` | Unix timestamp in seconds | Get the current block timestamp |
| `get_contract_id` | `() -> i32` | Pointer to contract ID in guest memory | Get the ID of the currently executing contract |
| `get_contract_id_len` | `() -> i32` | Length of contract ID string | Get the byte length of the contract ID |
| `get_input` | `() -> i32` | Pointer to JSON input in guest memory | Get the serialized input parameters |
| `get_input_len` | `() -> i32` | Length of input JSON bytes | Get the byte length of the input |

### 3. I/O Functions

Logging, events, return data, and error reporting.

| Function | Signature | Returns | Description |
|---|---|---|---|
| `log` | `(msg_ptr: i32, msg_len: i32)` | (void) | Log a debug message (max 64 logs per execution) |
| `emit_event` | `(name_ptr: i32, name_len: i32, data_ptr: i32, data_len: i32) -> i32` | `0` on success, `-1` if name too long, `-2` if data too long, `-3` if event limit reached | Emit a named event with JSON data (max 64 events per execution) |
| `set_return_data` | `(data_ptr: i32, data_len: i32)` | (void) | Set the return data for the contract call (max 65,536 bytes) |
| `set_error` | `(msg_ptr: i32, msg_len: i32)` | (void) | Set an error message (used by SDK macros on contract error) |

**Limits:**
- Maximum logs per execution: 64
- Maximum events per execution: 64
- Maximum event name length: 256 bytes
- Maximum event data length: 8,192 bytes
- Maximum return data: 65,536 bytes

### 4. Crypto Functions

Cryptographic hash operations.

| Function | Signature | Returns | Description |
|---|---|---|---|
| `sha256` | `(data_ptr: i32, data_len: i32) -> i32` | Pointer to 32-byte hash in guest memory | Compute SHA-256 hash of input data |
| `keccak256` | `(data_ptr: i32, data_len: i32) -> i32` | Pointer to 32-byte hash in guest memory | Compute Keccak-256 hash of input data |

### 5. Ledger Functions

Interact with the Rinku ledger: query accounts, transfer tokens, emit view keys.

| Function | Signature | Returns | Description |
|---|---|---|---|
| `get_balance` | `(addr_ptr: i32, addr_len: i32) -> i64` | Balance in micro-RKU (1 RKU = 1,000,000 micro), or `-1` if account not found | Get an account's balance |
| `get_staked` | `(addr_ptr: i32, addr_len: i32) -> i64` | Staked amount in micro-RKU, or `-1` if not found | Get an account's staked amount |
| `get_account_age` | `(addr_ptr: i32, addr_len: i32) -> i64` | Account age in seconds (current_timestamp - first_seen), or `-1` if not found | Get how long an account has existed |
| `get_nonce` | `(addr_ptr: i32, addr_len: i32) -> i64` | Current nonce, or `-1` if not found | Get an account's current nonce |
| `transfer` | `(from_ptr: i32, from_len: i32, to_ptr: i32, to_len: i32, amount_micro: i64) -> i32` | See error codes below | Transfer tokens between accounts |
| `emit_view_key` | `(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32) -> i32` | `0` on success, `-1` if key too long, `-2` if value too long, `-3` if limit reached | Emit a view key value for stateful receipts |
| `get_gas_price` | `() -> i64` | Current gas price in micro-RKU | Get the current network gas price |

#### Transfer Error Codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `-1` | Memory read error (invalid pointers) |
| `-2` | Invalid amount (zero or negative) |
| `-3` | Unauthorized (sender is not the caller) |
| `-4` | Insufficient balance |
| `-5` | Transfer limit exceeded (max 16 transfers per execution) |

#### View Key Limits
- Maximum view key emissions per execution: 32
- Maximum key length: 256 bytes
- Maximum value length: 8,192 bytes

## Gas Metering

Contracts are metered using two complementary mechanisms:

1. **wasmi Fuel**: The WASM interpreter charges fuel for each instruction executed. When fuel runs out, execution halts with an out-of-gas error.
2. **Explicit GasMeter**: Host functions charge additional gas for expensive operations (storage reads/writes, transfers, crypto operations).

Total gas used = wasmi fuel consumed + GasMeter charges.

The default gas limit is 10,000,000 units.

## Contract Entry Points

Contracts must export functions with the signature `() -> i32`. A return value of `0` indicates success; any non-zero value indicates an error.

### SDK Macros

The `rinku-contract-sdk` crate provides three macros for defining entry points:

```rust
use rinku_contract_sdk::*;

// For the init function (called on deploy)
contract_init!(my_init);
fn my_init(input: serde_json::Value) -> ContractResult {
    // Initialize contract state
    storage_set("owner", &get_caller())?;
    Ok(())
}

// For named call functions
contract_call!(my_function, handle_call);
fn handle_call(input: serde_json::Value) -> ContractResult {
    let caller = get_caller();
    let amount = input["amount"].as_f64().unwrap_or(0.0);
    transfer("recipient_addr", amount)?;
    Ok(())
}

// For a generic entrypoint
entrypoint!(process);
fn process(input: serde_json::Value) -> ContractResult {
    log("Processing...");
    Ok(())
}
```

## SDK Helper Functions

### Storage Helpers

```rust
// Typed get/set (any Serialize/Deserialize type)
storage_set("counter", &42u64)?;
let val: Option<u64> = storage_get("counter")?;
let val: u64 = storage_get_or_default("counter"); // returns 0 if missing

// Convenience getters
let n: u64 = storage_get_u64("counter");
let s: Option<String> = storage_get_string("name");
let b: bool = storage_get_bool("active");

// Atomic increment
let new_val: i64 = storage_increment("counter", 1)?;

// Check and delete
if storage_has("temp_key") {
    storage_delete("temp_key");
}
```

### Ledger Helpers

```rust
// Balance queries (returns f64 in RKU, not micro)
let bal: f64 = get_balance("some_address");
let bal_micro: i64 = get_balance_micro("some_address");
let staked: f64 = get_staked("some_address");

// Transfers (amount in RKU, not micro)
transfer("recipient", 5.0)?;                    // from caller
transfer_from("sender", "recipient", 5.0)?;     // explicit sender

// View keys for stateful receipts
emit_view_key("balance", &serde_json::json!(500))?;

// Events
emit_event("Transfer", EventData::new()
    .with("from", "alice")
    .with("to", "bob")
    .with("amount", 5.0))?;

// Return data
set_return_json(&serde_json::json!({"status": "ok"}));

// Guards
require(amount > 0.0, "Amount must be positive")?;
require_caller("owner_address")?;
```

### ContractError Builders

```rust
ContractError::invalid_input("Missing required field")  // code 1
ContractError::insufficient_balance()                     // code 2
ContractError::unauthorized()                             // code 3
ContractError::not_found("Token not found")              // code 4
ContractError::internal("Unexpected state")              // code 5
ContractError::storage_error("Write failed")             // code 6
ContractError::overflow()                                 // code 7
ContractError::new(42, "Custom error")                   // custom code
```

## Execution Output

After WASM execution, the runtime produces a `WasmExecutionOutput` containing:

- **result**: `ExecutionResult` with success/error, state diff, gas used, logs, events
- **transfer_ops**: List of `TransferOp` (from, to, amount) staged during execution
- **view_key_emissions**: List of `ViewKeyEmission` (key, value) for stateful receipts
- **return_data**: Raw bytes set via `set_return_data`

Transfer effects are applied to the ledger state after successful execution. View key values are included in the contract receipt for client-side verification.

## Build Target

Contracts must be compiled to `wasm32-unknown-unknown`:

```toml
# In your contract's Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
rinku-contract-sdk = { path = "../rinku-contract-sdk" }

# Build with:
# cargo build --target wasm32-unknown-unknown --release
```
