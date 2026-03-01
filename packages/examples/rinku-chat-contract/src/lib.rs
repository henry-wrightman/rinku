use rinku_contract_sdk::*;
use serde::{Deserialize, Serialize};

const MAX_ROOMS: usize = 50;
const DEFAULT_CAPACITY: u64 = 50;
const MAX_MESSAGE_LEN: usize = 500;
const MAX_ROOM_NAME_LEN: usize = 32;
const MAX_MEMBERS_PER_ROOM: usize = 200;

#[derive(Serialize, Deserialize, Clone, Debug)]
struct RoomMeta {
    id: String,
    name: String,
    owner: String,
    created_at: u64,
    member_count: u64,
    message_count: u64,
    capacity: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Message {
    seq: u64,
    author: String,
    ts: u64,
    content: String,
}

fn room_meta_key(id: &str) -> String {
    format!("room:{}:meta", id)
}

fn room_members_key(id: &str) -> String {
    format!("room:{}:members", id)
}

fn room_msg_key(id: &str, seq: u64, capacity: u64) -> String {
    let slot = seq % capacity;
    format!("room:{}:msg:{}", id, slot)
}

fn rooms_list_key() -> String {
    "rooms:all".to_string()
}

fn get_rooms_list() -> Vec<String> {
    storage_get_or_default::<Vec<String>>(&rooms_list_key())
}

fn set_rooms_list(rooms: &Vec<String>) -> ContractResult {
    storage_set(&rooms_list_key(), rooms)
}

fn get_room_meta(id: &str) -> Result<Option<RoomMeta>, ContractError> {
    storage_get::<RoomMeta>(&room_meta_key(id))
}

fn set_room_meta(meta: &RoomMeta) -> ContractResult {
    storage_set(&room_meta_key(&meta.id), meta)
}

fn get_members(id: &str) -> Vec<String> {
    storage_get_or_default::<Vec<String>>(&room_members_key(id))
}

fn set_members(id: &str, members: &Vec<String>) -> ContractResult {
    storage_set(&room_members_key(id), members)
}

fn is_member(id: &str, addr: &str) -> bool {
    let members = get_members(id);
    members.iter().any(|m| m == addr)
}

fn get_recent_messages(id: &str, meta: &RoomMeta, limit: u64) -> Vec<Message> {
    let count = meta.message_count;
    let cap = meta.capacity;
    if count == 0 {
        return vec![];
    }

    let fetch_count = limit.min(count).min(cap);
    let start_seq = if count > fetch_count { count - fetch_count } else { 0 };

    let mut msgs = Vec::new();
    for seq in start_seq..count {
        if let Ok(Some(msg)) = storage_get::<Message>(&room_msg_key(id, seq, cap)) {
            msgs.push(msg);
        }
    }
    msgs
}

fn emit_room_state(id: &str, meta: &RoomMeta, include_messages: bool) {
    let members = get_members(id);
    let mut room_state = serde_json::json!({
        "room": meta,
        "members": members,
    });

    if include_messages {
        let messages = get_recent_messages(id, meta, 20);
        room_state["messages"] = serde_json::json!(messages);
    }

    let _ = emit_view_key(&format!("room:{}", id), &room_state);
}

fn handle_init(_input: serde_json::Value) -> ContractResult {
    let rooms: Vec<String> = vec![];
    set_rooms_list(&rooms)?;
    log("chat contract initialized");
    set_return_json(&serde_json::json!({"status": "initialized"}));
    Ok(())
}

fn handle_create_room(input: serde_json::Value) -> ContractResult {
    let caller = get_caller();
    let id = input["id"].as_str()
        .ok_or(ContractError::invalid_input("missing 'id'"))?;
    let name = input["name"].as_str().unwrap_or(id);

    require(id.len() <= MAX_ROOM_NAME_LEN, "room id too long")?;
    require(!id.is_empty(), "room id cannot be empty")?;
    require(name.len() <= MAX_ROOM_NAME_LEN, "room name too long")?;

    let mut rooms = get_rooms_list();
    require(rooms.len() < MAX_ROOMS, "max rooms reached")?;

    if let Ok(Some(_)) = get_room_meta(id) {
        return Err(ContractError::invalid_input("room already exists"));
    }

    let capacity = input["capacity"].as_u64().unwrap_or(DEFAULT_CAPACITY).max(1).min(200);
    let ts = get_timestamp();

    let meta = RoomMeta {
        id: id.to_string(),
        name: name.to_string(),
        owner: caller.clone(),
        created_at: ts,
        member_count: 1,
        message_count: 0,
        capacity,
    };

    set_room_meta(&meta)?;
    let members = vec![caller.clone()];
    set_members(id, &members)?;

    rooms.push(id.to_string());
    set_rooms_list(&rooms)?;

    emit_room_state(id, &meta, false);
    let _ = emit_event("room_created", EventData::new()
        .with("id", id.to_string())
        .with("owner", caller));

    log(&format!("room '{}' created", id));
    set_return_json(&serde_json::json!({"status": "created", "room": meta}));
    Ok(())
}

fn handle_join_room(input: serde_json::Value) -> ContractResult {
    let caller = get_caller();
    let id = input["id"].as_str()
        .ok_or(ContractError::invalid_input("missing 'id'"))?;

    let mut meta = get_room_meta(id)?
        .ok_or(ContractError::not_found("room not found"))?;

    require(!is_member(id, &caller), "already a member")?;

    let mut members = get_members(id);
    require(members.len() < MAX_MEMBERS_PER_ROOM, "room is full")?;

    members.push(caller.clone());
    set_members(id, &members)?;

    meta.member_count = members.len() as u64;
    set_room_meta(&meta)?;

    emit_room_state(id, &meta, true);
    let _ = emit_event("member_joined", EventData::new()
        .with("room", id.to_string())
        .with("member", caller));

    set_return_json(&serde_json::json!({
        "status": "joined",
        "room": meta,
        "messages": get_recent_messages(id, &meta, 20),
    }));
    Ok(())
}

fn handle_leave_room(input: serde_json::Value) -> ContractResult {
    let caller = get_caller();
    let id = input["id"].as_str()
        .ok_or(ContractError::invalid_input("missing 'id'"))?;

    let mut meta = get_room_meta(id)?
        .ok_or(ContractError::not_found("room not found"))?;

    require(is_member(id, &caller), "not a member")?;

    let mut members = get_members(id);
    members.retain(|m| m != &caller);
    set_members(id, &members)?;

    meta.member_count = members.len() as u64;
    set_room_meta(&meta)?;

    emit_room_state(id, &meta, false);
    let _ = emit_event("member_left", EventData::new()
        .with("room", id.to_string())
        .with("member", caller));

    set_return_json(&serde_json::json!({"status": "left", "room": meta}));
    Ok(())
}

fn handle_send_message(input: serde_json::Value) -> ContractResult {
    let caller = get_caller();
    let id = input["id"].as_str()
        .ok_or(ContractError::invalid_input("missing 'id'"))?;
    let content = input["content"].as_str()
        .ok_or(ContractError::invalid_input("missing 'content'"))?;

    require(!content.is_empty(), "message cannot be empty")?;
    require(content.len() <= MAX_MESSAGE_LEN, "message too long")?;

    let mut meta = get_room_meta(id)?
        .ok_or(ContractError::not_found("room not found"))?;

    require(is_member(id, &caller), "not a member of this room")?;

    let ts = get_timestamp();
    let seq = meta.message_count;

    let msg = Message {
        seq,
        author: caller.clone(),
        ts,
        content: content.to_string(),
    };

    storage_set(&room_msg_key(id, seq, meta.capacity), &msg)?;
    meta.message_count = seq + 1;
    set_room_meta(&meta)?;

    emit_room_state(id, &meta, true);
    let _ = emit_event("message_sent", EventData::new()
        .with("room", id.to_string())
        .with("author", caller)
        .with("seq", seq));

    set_return_json(&serde_json::json!({
        "status": "sent",
        "seq": seq,
        "room": meta,
        "messages": get_recent_messages(id, &meta, 20),
    }));
    Ok(())
}

fn handle_get_room(input: serde_json::Value) -> ContractResult {
    let id = input["id"].as_str()
        .ok_or(ContractError::invalid_input("missing 'id'"))?;
    let limit = input["limit"].as_u64().unwrap_or(20);

    let meta = get_room_meta(id)?
        .ok_or(ContractError::not_found("room not found"))?;
    let members = get_members(id);
    let messages = get_recent_messages(id, &meta, limit);

    emit_room_state(id, &meta, true);

    set_return_json(&serde_json::json!({
        "room": meta,
        "members": members,
        "messages": messages,
    }));
    Ok(())
}

fn handle_list_rooms(_input: serde_json::Value) -> ContractResult {
    let room_ids = get_rooms_list();
    let mut rooms_info: Vec<serde_json::Value> = Vec::new();

    for rid in &room_ids {
        if let Ok(Some(meta)) = get_room_meta(rid) {
            rooms_info.push(serde_json::json!(meta));
        }
    }

    let _ = emit_view_key("rooms:list", &serde_json::json!(rooms_info));

    set_return_json(&serde_json::json!({
        "rooms": rooms_info,
    }));
    Ok(())
}

contract_init!(handle_init);
contract_call!(create_room, handle_create_room);
contract_call!(join_room, handle_join_room);
contract_call!(leave_room, handle_leave_room);
contract_call!(send_message, handle_send_message);
contract_call!(get_room, handle_get_room);
contract_call!(list_rooms, handle_list_rooms);
