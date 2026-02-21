use rinku_contract_sdk::*;

contract_init!(handle_init);
contract_call!(increment, handle_increment);
contract_call!(decrement, handle_decrement);
contract_call!(get_count, handle_get_count);

fn handle_init(_input: serde_json::Value) -> Result<(), ContractError> {
    log("counter: initializing");
    storage_set("count", &serde_json::json!(0));
    emit_view_key("count", "0");
    emit_event("initialized", &serde_json::json!({"count": 0}));
    set_return_json(&serde_json::json!({"status": "ok", "count": 0}));
    Ok(())
}

fn handle_increment(input: serde_json::Value) -> Result<(), ContractError> {
    let amount = input.get("amount")
        .and_then(|v| v.as_u64())
        .unwrap_or(1);

    let current = storage_get_u64("count").unwrap_or(0);
    let new_count = current + amount;

    storage_set("count", &serde_json::json!(new_count));
    emit_view_key("count", &format!("{}", new_count));

    let caller = get_caller();
    log(&format!("counter: {} incremented by {} (now {})", caller, amount, new_count));
    emit_event("incremented", &serde_json::json!({
        "caller": caller,
        "amount": amount,
        "new_count": new_count
    }));

    set_return_json(&serde_json::json!({"status": "ok", "count": new_count}));
    Ok(())
}

fn handle_decrement(input: serde_json::Value) -> Result<(), ContractError> {
    let amount = input.get("amount")
        .and_then(|v| v.as_u64())
        .unwrap_or(1);

    let current = storage_get_u64("count").unwrap_or(0);
    if amount > current {
        return Err(ContractError::new(1, "cannot decrement below zero"));
    }
    let new_count = current - amount;

    storage_set("count", &serde_json::json!(new_count));
    emit_view_key("count", &format!("{}", new_count));

    let caller = get_caller();
    log(&format!("counter: {} decremented by {} (now {})", caller, amount, new_count));
    emit_event("decremented", &serde_json::json!({
        "caller": caller,
        "amount": amount,
        "new_count": new_count
    }));

    set_return_json(&serde_json::json!({"status": "ok", "count": new_count}));
    Ok(())
}

fn handle_get_count(_input: serde_json::Value) -> Result<(), ContractError> {
    let current = storage_get_u64("count").unwrap_or(0);
    log(&format!("counter: current count is {}", current));
    set_return_json(&serde_json::json!({"count": current}));
    Ok(())
}
