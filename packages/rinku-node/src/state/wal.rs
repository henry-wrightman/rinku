use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use tracing::{info, warn};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum WalEntry {
    BeginCheckpoint {
        height: u64,
        checkpoint_hash: String,
        timestamp_ms: u64,
    },
    AccountUpdate {
        address: String,
        balance: u64,
        nonce: u64,
        staked: u64,
    },
    ValidatorUpdate {
        address: String,
        stake: u64,
    },
    CommitCheckpoint {
        height: u64,
        checkpoint_hash: String,
    },
    RollbackCheckpoint {
        height: u64,
    },
}

pub struct WriteAheadLog {
    path: PathBuf,
    file: Option<std::fs::File>,
    current_height: Option<u64>,
    committed_height: u64,
    entry_count: u64,
}

impl WriteAheadLog {
    pub fn new(data_dir: &str) -> Self {
        let path = PathBuf::from(data_dir).join("wal.log");
        Self {
            path,
            file: None,
            current_height: None,
            committed_height: 0,
            entry_count: 0,
        }
    }

    pub fn open(&mut self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("WAL: failed to create dir: {}", e))?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("WAL: failed to open: {}", e))?;
        self.file = Some(file);
        info!("WAL: opened at {:?}", self.path);
        Ok(())
    }

    pub fn write_entry(&mut self, entry: &WalEntry) -> Result<(), String> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| "WAL: not opened".to_string())?;
        let json =
            serde_json::to_string(entry).map_err(|e| format!("WAL: serialize error: {}", e))?;
        writeln!(file, "{}", json).map_err(|e| format!("WAL: write error: {}", e))?;
        file.flush()
            .map_err(|e| format!("WAL: flush error: {}", e))?;
        self.entry_count += 1;

        match entry {
            WalEntry::BeginCheckpoint { height, .. } => {
                self.current_height = Some(*height);
            }
            WalEntry::CommitCheckpoint { height, .. } => {
                self.committed_height = *height;
                self.current_height = None;
            }
            WalEntry::RollbackCheckpoint { .. } => {
                self.current_height = None;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn begin_checkpoint(&mut self, height: u64, checkpoint_hash: &str) -> Result<(), String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.write_entry(&WalEntry::BeginCheckpoint {
            height,
            checkpoint_hash: checkpoint_hash.to_string(),
            timestamp_ms: now_ms,
        })
    }

    pub fn log_account_update(
        &mut self,
        address: &str,
        balance: u64,
        nonce: u64,
        staked: u64,
    ) -> Result<(), String> {
        self.write_entry(&WalEntry::AccountUpdate {
            address: address.to_string(),
            balance,
            nonce,
            staked,
        })
    }

    pub fn commit_checkpoint(&mut self, height: u64, checkpoint_hash: &str) -> Result<(), String> {
        self.write_entry(&WalEntry::CommitCheckpoint {
            height,
            checkpoint_hash: checkpoint_hash.to_string(),
        })?;
        self.truncate()
    }

    pub fn truncate(&mut self) -> Result<(), String> {
        if let Some(ref mut file) = self.file {
            drop(std::mem::replace(
                file,
                std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&self.path)
                    .map_err(|e| format!("WAL: truncate reopen error: {}", e))?,
            ));
        }
        self.entry_count = 0;
        Ok(())
    }

    pub fn recover(&mut self) -> Result<Option<WalRecoveryData>, String> {
        if !self.path.exists() {
            info!("WAL: no WAL file found at {:?}, clean startup", self.path);
            return Ok(None);
        }

        let file = std::fs::File::open(&self.path)
            .map_err(|e| format!("WAL: failed to open for recovery: {}", e))?;
        let reader = BufReader::new(file);

        let mut entries: Vec<WalEntry> = Vec::new();
        for line in reader.lines() {
            let line = line.map_err(|e| format!("WAL: read error during recovery: {}", e))?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<WalEntry>(&line) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    warn!("WAL: corrupt entry during recovery (skipping): {}", e);
                    break;
                }
            }
        }

        if entries.is_empty() {
            info!("WAL: empty WAL file, clean startup");
            return Ok(None);
        }

        let mut uncommitted_begin: Option<(u64, String)> = None;
        let mut account_updates: Vec<(String, u64, u64, u64)> = Vec::new();
        let mut last_committed_height: u64 = 0;

        for entry in &entries {
            match entry {
                WalEntry::BeginCheckpoint {
                    height,
                    checkpoint_hash,
                    ..
                } => {
                    uncommitted_begin = Some((*height, checkpoint_hash.clone()));
                    account_updates.clear();
                }
                WalEntry::AccountUpdate {
                    address,
                    balance,
                    nonce,
                    staked,
                } => {
                    account_updates.push((address.clone(), *balance, *nonce, *staked));
                }
                WalEntry::CommitCheckpoint { height, .. } => {
                    last_committed_height = *height;
                    uncommitted_begin = None;
                    account_updates.clear();
                }
                WalEntry::RollbackCheckpoint { .. } => {
                    uncommitted_begin = None;
                    account_updates.clear();
                }
                WalEntry::ValidatorUpdate { .. } => {}
            }
        }

        if let Some((height, hash)) = uncommitted_begin {
            if !account_updates.is_empty() {
                info!(
                    "WAL RECOVERY: found uncommitted checkpoint at h={} with {} account updates — replaying",
                    height, account_updates.len()
                );
                return Ok(Some(WalRecoveryData {
                    height,
                    checkpoint_hash: hash,
                    account_updates,
                    action: WalRecoveryAction::Replay,
                }));
            } else {
                info!(
                    "WAL RECOVERY: found BeginCheckpoint at h={} with no account updates — rolling back",
                    height
                );
                return Ok(Some(WalRecoveryData {
                    height,
                    checkpoint_hash: hash,
                    account_updates: vec![],
                    action: WalRecoveryAction::Rollback,
                }));
            }
        }

        info!(
            "WAL RECOVERY: all entries committed (last h={}), clean startup",
            last_committed_height
        );
        Ok(None)
    }

    pub fn is_open(&self) -> bool {
        self.file.is_some()
    }

    pub fn committed_height(&self) -> u64 {
        self.committed_height
    }
}

#[derive(Debug)]
pub struct WalRecoveryData {
    pub height: u64,
    pub checkpoint_hash: String,
    pub account_updates: Vec<(String, u64, u64, u64)>,
    pub action: WalRecoveryAction,
}

#[derive(Debug)]
pub enum WalRecoveryAction {
    Replay,
    Rollback,
}
