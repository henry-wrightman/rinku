use std::sync::Arc;
use sysinfo::System;
use crate::state::NodeState;
use crate::gossip::GossipService;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Network,
    Validator,
    DAG,
    Logs,
}

impl Tab {
    pub fn all() -> &'static [Tab] {
        &[Tab::Dashboard, Tab::Network, Tab::Validator, Tab::DAG, Tab::Logs]
    }

    pub fn title(&self) -> &'static str {
        match self {
            Tab::Dashboard => "Dashboard",
            Tab::Network => "Network",
            Tab::Validator => "Validator",
            Tab::DAG => "DAG",
            Tab::Logs => "Logs",
        }
    }

    pub fn next(&self) -> Tab {
        match self {
            Tab::Dashboard => Tab::Network,
            Tab::Network => Tab::Validator,
            Tab::Validator => Tab::DAG,
            Tab::DAG => Tab::Logs,
            Tab::Logs => Tab::Dashboard,
        }
    }

    pub fn prev(&self) -> Tab {
        match self {
            Tab::Dashboard => Tab::Logs,
            Tab::Network => Tab::Dashboard,
            Tab::Validator => Tab::Network,
            Tab::DAG => Tab::Validator,
            Tab::Logs => Tab::DAG,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeStats {
    pub cpu_usage: f32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub uptime_secs: u64,
    pub process_memory_mb: u64,
}

#[derive(Debug, Clone)]
pub struct NetworkStats {
    pub peer_count: usize,
    pub peers: Vec<String>,
    pub tps: f64,
    pub total_transactions: u64,
    pub finalized_count: u64,
    pub pending_count: u64,
    pub checkpoint_height: u64,
    pub latest_checkpoint_id: Option<String>,
    pub gas_price: f64,
    pub total_burned: f64,
}

#[derive(Debug, Clone)]
pub struct StakingConfig {
    pub min_stake_amount: f64,
    pub unbonding_period_days: u32,
}

#[derive(Debug, Clone)]
pub struct ValidatorStats {
    pub address: Option<String>,
    pub is_validator: bool,
    pub stake_amount: f64,
    pub pending_rewards: f64,
    pub total_validators: usize,
    pub total_staked: f64,
    pub unbonding_amount: f64,
    pub config: StakingConfig,
}

#[derive(Debug, Clone)]
pub struct DagStats {
    pub tip_count: usize,
    pub tips: Vec<String>,
    pub dag_size: usize,
    pub recent_txs: Vec<RecentTx>,
}

#[derive(Debug, Clone)]
pub struct RecentTx {
    pub hash: String,
    pub from: String,
    pub to: String,
    pub amount: f64,
    pub finalized: bool,
}

pub struct App {
    pub running: bool,
    pub current_tab: Tab,
    pub node_stats: NodeStats,
    pub network_stats: NetworkStats,
    pub validator_stats: ValidatorStats,
    pub dag_stats: DagStats,
    pub logs: Vec<String>,
    pub scroll_offset: usize,
    pub state: Arc<NodeState>,
    pub gossip_service: Option<Arc<GossipService>>,
    pub system: System,
    pub start_time: std::time::Instant,
    pub node_id: String,
}

impl App {
    pub fn new(state: Arc<NodeState>, gossip_service: Option<Arc<GossipService>>, node_id: String) -> Self {
        Self {
            running: true,
            current_tab: Tab::Dashboard,
            node_stats: NodeStats {
                cpu_usage: 0.0,
                memory_used_mb: 0,
                memory_total_mb: 0,
                uptime_secs: 0,
                process_memory_mb: 0,
            },
            network_stats: NetworkStats {
                peer_count: 0,
                peers: vec![],
                tps: 0.0,
                total_transactions: 0,
                finalized_count: 0,
                pending_count: 0,
                checkpoint_height: 0,
                latest_checkpoint_id: None,
                gas_price: 0.001,
                total_burned: 0.0,
            },
            validator_stats: ValidatorStats {
                address: None,
                is_validator: false,
                stake_amount: 0.0,
                pending_rewards: 0.0,
                total_validators: 0,
                total_staked: 0.0,
                unbonding_amount: 0.0,
                config: StakingConfig {
                    min_stake_amount: 100.0,
                    unbonding_period_days: 14,
                },
            },
            dag_stats: DagStats {
                tip_count: 0,
                tips: vec![],
                dag_size: 0,
                recent_txs: vec![],
            },
            logs: vec![
                "Rinku Node TUI started".to_string(),
                "Press Tab to switch views, q to quit".to_string(),
            ],
            scroll_offset: 0,
            state,
            gossip_service,
            system: System::new_all(),
            start_time: std::time::Instant::now(),
            node_id,
        }
    }

    pub fn next_tab(&mut self) {
        self.current_tab = self.current_tab.next();
        self.scroll_offset = 0;
    }

    pub fn prev_tab(&mut self) {
        self.current_tab = self.current_tab.prev();
        self.scroll_offset = 0;
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    pub fn quit(&mut self) {
        self.running = false;
    }

    pub async fn update(&mut self) {
        self.system.refresh_all();

        let uptime = self.start_time.elapsed().as_secs();
        let cpu_usage = self.system.global_cpu_usage();
        let memory_used = self.system.used_memory() / 1024 / 1024;
        let memory_total = self.system.total_memory() / 1024 / 1024;

        let pid = sysinfo::get_current_pid().ok();
        let process_memory = pid
            .and_then(|p| self.system.process(p))
            .map(|p| p.memory() / 1024 / 1024)
            .unwrap_or(0);

        self.node_stats = NodeStats {
            cpu_usage,
            memory_used_mb: memory_used,
            memory_total_mb: memory_total,
            uptime_secs: uptime,
            process_memory_mb: process_memory,
        };

        let dashboard = self.state.get_dashboard_stats().await;
        let tips = self.state.get_tips().await;
        let (dag_size, finalized, pending) = self.state.get_dag_stats().await;
        let checkpoint_height = self.state.get_checkpoint_height();
        let gas_price = self.state.get_gas_price().await;
        let (_, _, _, total_burned) = self.state.get_gas_stats().await;
        // Use rewards service for accurate staking data
        let rewards = self.state.rewards.read().await;
        let validator_count = rewards.get_active_validators().len();
        let total_staked = rewards.get_total_staked();
        drop(rewards);
        let tps = self.state.get_finalized_tps().await;

        let (dag_nodes, _total, _has_more) = self.state.get_dag_nodes_paginated(0, 10).await;
        let recent_txs: Vec<RecentTx> = dag_nodes
            .iter()
            .map(|node| RecentTx {
                hash: node.hash.chars().take(12).collect(),
                from: node.from.chars().take(8).collect(),
                to: node.to.chars().take(8).collect(),
                amount: node.amount,
                finalized: node.finalized,
            })
            .collect();

        // Fetch peer info from gossip service if available
        let (peer_count, peers) = if let Some(ref gs) = self.gossip_service {
            let count = gs.get_peer_count().await;
            let peer_list = gs.get_peer_addresses().await;
            (count, peer_list)
        } else {
            (0, vec![])
        };

        self.network_stats = NetworkStats {
            peer_count,
            peers,
            tps,
            total_transactions: dashboard.total_transactions,
            finalized_count: finalized as u64,
            pending_count: pending as u64,
            checkpoint_height,
            latest_checkpoint_id: self.state.get_latest_checkpoint_id().await,
            gas_price,
            total_burned,
        };

        // Get this node's validator info from state
        let (validator_address, _bls_key) = self.state.get_validator_info().await;
        let (stake_amount, pending_rewards, unbonding_amount, is_validator) = 
            if let Some(ref addr) = validator_address {
                self.state.get_validator_staking_info(addr).await
            } else {
                (0.0, 0.0, 0.0, false)
            };

        // Get staking config from node state
        let (min_stake, unbonding_days) = self.state.get_staking_config().await;

        self.validator_stats = ValidatorStats {
            address: validator_address,
            is_validator,
            stake_amount,
            pending_rewards,
            total_validators: validator_count,
            total_staked,
            unbonding_amount,
            config: StakingConfig {
                min_stake_amount: min_stake,
                unbonding_period_days: unbonding_days,
            },
        };

        self.dag_stats = DagStats {
            tip_count: tips.len(),
            tips: tips.iter().take(10).map(|t| t.chars().take(16).collect()).collect(),
            dag_size,
            recent_txs,
        };
    }

    pub fn add_log(&mut self, msg: String) {
        self.logs.push(format!("[{}] {}", chrono::Utc::now().format("%H:%M:%S"), msg));
        if self.logs.len() > 1000 {
            self.logs.remove(0);
        }
    }
}
