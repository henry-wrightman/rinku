use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct ContractError {
    pub code: i32,
    pub message: String,
}

impl ContractError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::new(1, msg)
    }

    pub fn insufficient_balance() -> Self {
        Self::new(2, "Insufficient balance")
    }

    pub fn unauthorized() -> Self {
        Self::new(3, "Unauthorized")
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(4, msg)
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(5, msg)
    }

    pub fn storage_error(msg: impl Into<String>) -> Self {
        Self::new(6, msg)
    }

    pub fn overflow() -> Self {
        Self::new(7, "Arithmetic overflow")
    }
}

impl core::fmt::Display for ContractError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "ContractError({}): {}", self.code, self.message)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferRequest {
    pub to: String,
    pub amount_micro: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventData {
    #[serde(flatten)]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

impl EventData {
    pub fn new() -> Self {
        Self {
            fields: serde_json::Map::new(),
        }
    }

    pub fn with(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.fields).unwrap_or_default()
    }
}

impl Default for EventData {
    fn default() -> Self {
        Self::new()
    }
}

pub type ContractResult = Result<(), ContractError>;
