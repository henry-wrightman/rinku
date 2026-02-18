(module
  ;; === Rinku Host Function Imports ===
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

  ;; === Memory (2 pages = 128KB) ===
  ;; Page 0 (0-65535): reserved for host alloc_offset writes
  ;; Page 1 (65536+): our static data
  (memory (export "memory") 2)

  ;; === Static Data (placed at 65536+ to avoid host alloc_offset overlap) ===
  ;; Storage key "count" at 65536, length 5
  (data (i32.const 65536) "count")
  ;; Value "0" at 65544
  (data (i32.const 65544) "0")
  ;; Log messages
  (data (i32.const 65552) "counter: initialized to 0")
  (data (i32.const 65584) "counter: incremented")
  (data (i32.const 65608) "counter: decremented")
  (data (i32.const 65632) "counter: read count")
  ;; Event names
  (data (i32.const 65664) "initialized")
  (data (i32.const 65680) "incremented")
  (data (i32.const 65696) "decremented")
  ;; JSON data
  (data (i32.const 65712) "{\"count\":0}")
  (data (i32.const 65728) "{\"status\":\"ok\"}")
  ;; View key name "count" at 65752
  (data (i32.const 65752) "count")
  ;; Error message
  (data (i32.const 65760) "cannot decrement below zero")
  ;; Scratch space for new digit at 65800
  ;; (data (i32.const 65800) "") ;; scratch byte

  ;; === init: set count to "0" ===
  (func (export "init") (result i32)
    ;; storage_write("count", "0")
    (call $storage_write
      (i32.const 65536) (i32.const 5)    ;; key: "count"
      (i32.const 65544) (i32.const 1)    ;; value: "0"
    )
    drop
    ;; log("counter: initialized to 0")
    (call $log (i32.const 65552) (i32.const 25))
    ;; emit_event("initialized", {"count":0})
    (call $emit_event
      (i32.const 65664) (i32.const 11)
      (i32.const 65712) (i32.const 11)
    )
    drop
    ;; emit_view_key("count", "0")
    (call $emit_view_key
      (i32.const 65752) (i32.const 5)
      (i32.const 65544) (i32.const 1)
    )
    drop
    ;; set_return_data({"status":"ok"})
    (call $set_return_data (i32.const 65728) (i32.const 14))
    (i32.const 0)
  )

  ;; === increment: add 1 to count ===
  (func (export "increment") (result i32)
    (local $current_ptr i32)
    (local $val i32)
    ;; Read current count length (also primes the read)
    (call $storage_read_len (i32.const 65536) (i32.const 5))
    drop
    ;; Read current value (returns ptr in page 0 area)
    (local.set $current_ptr
      (call $storage_read (i32.const 65536) (i32.const 5))
    )
    ;; Parse ASCII digit → numeric
    (local.set $val
      (i32.sub
        (i32.load8_u (local.get $current_ptr))
        (i32.const 48)
      )
    )
    ;; Increment
    (local.set $val (i32.add (local.get $val) (i32.const 1)))
    ;; Convert back to ASCII and store in scratch space at 65800
    (i32.store8 (i32.const 65800) (i32.add (local.get $val) (i32.const 48)))
    ;; storage_write("count", new_digit)
    (call $storage_write
      (i32.const 65536) (i32.const 5)
      (i32.const 65800) (i32.const 1)
    )
    drop
    ;; log("counter: incremented")
    (call $log (i32.const 65584) (i32.const 20))
    ;; emit_event("incremented", {"status":"ok"})
    (call $emit_event
      (i32.const 65680) (i32.const 11)
      (i32.const 65728) (i32.const 14)
    )
    drop
    ;; emit_view_key("count", new_digit)
    (call $emit_view_key
      (i32.const 65752) (i32.const 5)
      (i32.const 65800) (i32.const 1)
    )
    drop
    ;; set_return_data({"status":"ok"})
    (call $set_return_data (i32.const 65728) (i32.const 14))
    (i32.const 0)
  )

  ;; === decrement: subtract 1 from count ===
  (func (export "decrement") (result i32)
    (local $current_ptr i32)
    (local $val i32)
    ;; Read current count
    (call $storage_read_len (i32.const 65536) (i32.const 5))
    drop
    (local.set $current_ptr
      (call $storage_read (i32.const 65536) (i32.const 5))
    )
    (local.set $val
      (i32.sub
        (i32.load8_u (local.get $current_ptr))
        (i32.const 48)
      )
    )
    ;; Check if val == 0 → error
    (if (i32.eqz (local.get $val))
      (then
        (call $set_error (i32.const 65760) (i32.const 26))
        (return (i32.const 1))
      )
    )
    ;; Decrement
    (local.set $val (i32.sub (local.get $val) (i32.const 1)))
    ;; Store new value
    (i32.store8 (i32.const 65800) (i32.add (local.get $val) (i32.const 48)))
    (call $storage_write
      (i32.const 65536) (i32.const 5)
      (i32.const 65800) (i32.const 1)
    )
    drop
    ;; log
    (call $log (i32.const 65608) (i32.const 20))
    ;; emit_event
    (call $emit_event
      (i32.const 65696) (i32.const 11)
      (i32.const 65728) (i32.const 14)
    )
    drop
    ;; emit_view_key
    (call $emit_view_key
      (i32.const 65752) (i32.const 5)
      (i32.const 65800) (i32.const 1)
    )
    drop
    (call $set_return_data (i32.const 65728) (i32.const 14))
    (i32.const 0)
  )

  ;; === get_count: read and return current count ===
  (func (export "get_count") (result i32)
    (local $ptr i32)
    (local $len i32)
    (local.set $len
      (call $storage_read_len (i32.const 65536) (i32.const 5))
    )
    (local.set $ptr
      (call $storage_read (i32.const 65536) (i32.const 5))
    )
    ;; log
    (call $log (i32.const 65632) (i32.const 19))
    ;; Return the stored value
    (call $set_return_data (local.get $ptr) (local.get $len))
    (i32.const 0)
  )
)
