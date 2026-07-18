#!/bin/bash
#
# Rinku Local Multi-Node Testnet
#
# Spins up multiple Rinku nodes locally for testing peer synchronization,
# consensus, and multi-node validation.
#
# Usage:
#   ./scripts/local-testnet.sh start [NUM_NODES]  # Start testnet (default: 2 nodes)
#   ./scripts/local-testnet.sh stop               # Stop all nodes
#   ./scripts/local-testnet.sh status             # Check node status
#   ./scripts/local-testnet.sh validate           # Run multi-node validation
#   ./scripts/local-testnet.sh logs [NODE_NUM]    # View logs for a node
#
# Examples:
#   ./scripts/local-testnet.sh start 3           # Start 3-node testnet
#   ./scripts/local-testnet.sh validate          # Validate all nodes
#

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Configuration
# Use ports 3011+ to avoid conflict with main workflow on 3001/4001
BASE_PORT=3011
P2P_BASE_PORT=4011
NODE_PORTS=()
NUM_NODES=${2:-2}
LOG_DIR="$PROJECT_ROOT/.testnet-logs"
CHAIN_ID="rinku-testnet"
NETWORK_ID="testnet"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[OK]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Calculate ports for nodes
calculate_ports() {
    NODE_PORTS=()
    for ((i=0; i<NUM_NODES; i++)); do
        NODE_PORTS+=($((BASE_PORT + i)))
    done
}

# Build peer list for a specific node (excluding itself)
build_peer_list() {
    local node_index=$1
    local peers=""
    
    for ((i=0; i<NUM_NODES; i++)); do
        if [ $i -ne $node_index ]; then
            if [ -n "$peers" ]; then
                peers="$peers,"
            fi
            peers="${peers}http://localhost:${NODE_PORTS[$i]}"
        fi
    done
    
    echo "$peers"
}

wait_for_bootstrap_info() {
    local port=$1
    local max_attempts=30
    local attempt=0

    while [ $attempt -lt $max_attempts ]; do
        local response
        response="$(curl -s "http://localhost:${port}/api/bootstrap" 2>/dev/null)"
        if [ -n "$response" ] && echo "$response" | grep -q '"peerId"'; then
            echo "$response"
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 1
    done

    return 1
}

start_testnet() {
    calculate_ports
    
    echo ""
    echo "╔══════════════════════════════════════════════════════════════╗"
    echo "║           RINKU LOCAL TESTNET                                ║"
    echo "╚══════════════════════════════════════════════════════════════╝"
    echo ""
    log_info "Starting $NUM_NODES node testnet..."
    
    # Create log directory
    mkdir -p "$LOG_DIR"
    
    # Check if cargo is available
    if ! command -v cargo &> /dev/null; then
        log_error "Cargo not found. Please install Rust."
        exit 1
    fi
    
    # Build the node first (use debug build for faster iteration)
    log_info "Building Rinku node (debug mode for faster startup)..."
    cd "$PROJECT_ROOT"
    
    # Check if binary already exists and is recent
    if [ -f "target/debug/rinku-node" ]; then
        local binary_age=$(($(date +%s) - $(stat -c %Y target/debug/rinku-node 2>/dev/null || echo 0)))
        if [ $binary_age -lt 300 ]; then
            log_success "Using existing build (built ${binary_age}s ago)"
        else
            cargo build -p rinku-node || {
                log_error "Build failed"
                exit 1
            }
        fi
    else
        cargo build -p rinku-node || {
            log_error "Build failed"
            exit 1
        }
    fi
    
    # Start genesis node first (needed to fetch GENESIS_VALIDATORS for mainnet-mode validators)
    local genesis_port=${NODE_PORTS[0]}
    local genesis_log="$LOG_DIR/node-1.log"
    local genesis_db="$LOG_DIR/node-1-data"
    local genesis_p2p_port=$P2P_BASE_PORT
    local genesis_public_url="http://localhost:${genesis_port}"

    log_info "Starting Node 1 (genesis) on port $genesis_port..."
    rm -rf "$genesis_db"

    API_PORT=$genesis_port \
    P2P_PORT=$genesis_p2p_port \
    NODE_PEERS="$(build_peer_list 0)" \
    DATA_DIR="$genesis_db" \
    MAINNET_MODE=true \
    FAUCET_ENABLED=true \
    ALLOW_UNTRUSTED_GENESIS=true \
    CHAIN_ID="$CHAIN_ID" \
    NETWORK_ID="$NETWORK_ID" \
    VALIDATOR_KEY_PASSWORD="testnet-node-1" \
    PUBLIC_URL="$genesis_public_url" \
    RUST_LOG=info \
    ./target/debug/rinku-node &>"$genesis_log" &

    local genesis_pid=$!
    echo $genesis_pid > "$LOG_DIR/node-1.pid"
    log_success "Node 1 started (PID: $genesis_pid, Port: $genesis_port, P2P: $genesis_p2p_port)"
    sleep 2

    log_info "Fetching bootstrap info from genesis..."
    local bootstrap_info
    bootstrap_info="$(wait_for_bootstrap_info "$genesis_port")" || {
        log_error "Failed to get bootstrap info from genesis node"
        exit 1
    }

    local peer_id
    peer_id="$(echo "$bootstrap_info" | grep -o '"peerId":"[^"]*"' | cut -d'"' -f4)"
    local genesis_validator_env
    genesis_validator_env="$(echo "$bootstrap_info" | grep -o '"genesisValidatorEnv":"[^"]*"' | cut -d'"' -f4)"
    local listen_addr
    listen_addr="$(echo "$bootstrap_info" | grep -o '"listenAddr":"[^"]*"' | cut -d'"' -f4)"
    local p2p_port
    p2p_port="$(echo "$listen_addr" | sed -n 's|.*/tcp/\([0-9]*\).*|\1|p')"
    local bootstrap_peer="/ip4/127.0.0.1/tcp/${p2p_port}/p2p/${peer_id}"

    if [ -z "$genesis_validator_env" ] || [ -z "$peer_id" ] || [ -z "$p2p_port" ]; then
        log_error "Incomplete bootstrap info from genesis node"
        exit 1
    fi

    # Start remaining nodes with strict mainnet-mode requirements
    for ((i=1; i<NUM_NODES; i++)); do
        local port=${NODE_PORTS[$i]}
        local peers=$(build_peer_list $i)
        local log_file="$LOG_DIR/node-$((i+1)).log"
        local db_path="$LOG_DIR/node-$((i+1))-data"
        local p2p_port=$((P2P_BASE_PORT + i))
        local public_url="http://localhost:${port}"
        
        log_info "Starting Node $((i+1)) on port $port..."
        rm -rf "$db_path"
        
        API_PORT=$port \
        P2P_PORT=$p2p_port \
        NODE_PEERS="$peers" \
        P2P_BOOTSTRAP_PEERS="$bootstrap_peer" \
        GENESIS_VALIDATORS="$genesis_validator_env" \
        DATA_DIR="$db_path" \
        MAINNET_MODE=true \
        FAUCET_ENABLED=true \
        CHAIN_ID="$CHAIN_ID" \
        NETWORK_ID="$NETWORK_ID" \
        VALIDATOR_KEY_PASSWORD="testnet-node-$((i+1))" \
        PUBLIC_URL="$public_url" \
        RUST_LOG=info \
        ./target/debug/rinku-node &>"$log_file" &
        
        local pid=$!
        echo $pid > "$LOG_DIR/node-$((i+1)).pid"
        
        log_success "Node $((i+1)) started (PID: $pid, Port: $port, P2P: $p2p_port)"
        if [ -n "$peers" ]; then
            log_info "  Peers: $peers"
        else
            log_info "  Peers: None (bootstrap node)"
        fi
        sleep 2
    done
    
    echo ""
    log_info "Waiting for nodes to initialize..."
    sleep 3
    
    # Check status
    check_status
    
    echo ""
    log_success "Local testnet started!"
    echo ""
    echo "  Available endpoints:"
    for ((i=0; i<NUM_NODES; i++)); do
        echo "    Node $((i+1)): http://localhost:${NODE_PORTS[$i]}"
    done
    echo ""
    echo "  Commands:"
    echo "    ./scripts/local-testnet.sh status    - Check node status"
    echo "    ./scripts/local-testnet.sh validate  - Run multi-node validation"
    echo "    ./scripts/local-testnet.sh logs 1    - View Node 1 logs"
    echo "    ./scripts/local-testnet.sh stop      - Stop all nodes"
    echo ""
}

stop_testnet() {
    log_info "Stopping testnet..."
    
    local stopped=0
    
    for pid_file in "$LOG_DIR"/node-*.pid; do
        if [ -f "$pid_file" ]; then
            local pid=$(cat "$pid_file")
            local node_name=$(basename "$pid_file" .pid)
            
            if kill -0 $pid 2>/dev/null; then
                kill $pid 2>/dev/null || true
                log_success "Stopped $node_name (PID: $pid)"
                stopped=$((stopped + 1))
            fi
            rm -f "$pid_file"
        fi
    done
    
    if [ $stopped -eq 0 ]; then
        log_info "No running nodes found"
    else
        log_success "Stopped $stopped nodes"
    fi
}

check_status() {
    calculate_ports
    
    echo ""
    echo "  Node Status:"
    echo "  ─────────────────────────────────────────────────────"
    
    local running=0
    local total=0
    
    for pid_file in "$LOG_DIR"/node-*.pid; do
        if [ -f "$pid_file" ]; then
            total=$((total + 1))
            local pid=$(cat "$pid_file")
            local node_num=$(basename "$pid_file" .pid | sed 's/node-//')
            local port=$((BASE_PORT + node_num - 1))
            
            if kill -0 $pid 2>/dev/null; then
                # Check if HTTP is responding
                if curl -s "http://localhost:$port/api/sync/status" >/dev/null 2>&1; then
                    local status=$(curl -s "http://localhost:$port/api/sync/status")
                    local dag_size=$(echo "$status" | grep -o '"dagSize":[0-9]*' | cut -d':' -f2)
                    log_success "Node $node_num: Running (Port: $port, DAG: ${dag_size:-?} txs)"
                    running=$((running + 1))
                else
                    log_warn "Node $node_num: Starting... (Port: $port)"
                fi
            else
                log_error "Node $node_num: Stopped (Port: $port)"
            fi
        fi
    done
    
    if [ $total -eq 0 ]; then
        log_info "No testnet nodes configured. Run './scripts/local-testnet.sh start' first."
    else
        echo "  ─────────────────────────────────────────────────────"
        echo "  Running: $running/$total nodes"
    fi
    echo ""
}

view_logs() {
    local node_num=${1:-1}
    local log_file="$LOG_DIR/node-$node_num.log"
    
    if [ ! -f "$log_file" ]; then
        log_error "Log file not found for Node $node_num"
        exit 1
    fi
    
    log_info "Showing logs for Node $node_num (Ctrl+C to exit)..."
    tail -f "$log_file"
}

run_validation() {
    # Detect running nodes from PID files (not hardcoded NUM_NODES)
    local urls=""
    local running_count=0
    
    for pid_file in "$LOG_DIR"/node-*.pid; do
        if [ -f "$pid_file" ]; then
            local pid=$(cat "$pid_file")
            local node_num=$(basename "$pid_file" .pid | sed 's/node-//')
            local port=$((BASE_PORT + node_num - 1))
            
            # Check if process is actually running
            if kill -0 "$pid" 2>/dev/null; then
                if [ -n "$urls" ]; then
                    urls="$urls "
                fi
                urls="${urls}http://localhost:$port"
                ((running_count++))
            fi
        fi
    done
    
    if [ -z "$urls" ] || [ "$running_count" -lt 2 ]; then
        log_error "Need at least 2 running nodes to validate. Found: $running_count"
        log_info "Start testnet first: ./scripts/local-testnet.sh start 3"
        exit 1
    fi
    
    log_info "Running multi-node validation on $running_count nodes..."
    npx ts-node "$SCRIPT_DIR/validate-multi-node.ts" $urls
}

# Main command handler
case "${1:-}" in
    start)
        start_testnet
        ;;
    stop)
        stop_testnet
        ;;
    status)
        check_status
        ;;
    validate)
        run_validation
        ;;
    logs)
        view_logs "${2:-1}"
        ;;
    *)
        echo ""
        echo "Rinku Local Testnet Manager"
        echo ""
        echo "Usage: $0 <command> [options]"
        echo ""
        echo "Commands:"
        echo "  start [NUM_NODES]   Start a local testnet (default: 2 nodes)"
        echo "  stop                Stop all testnet nodes"
        echo "  status              Show status of all nodes"
        echo "  validate            Run multi-node validation"
        echo "  logs [NODE_NUM]     View logs for a specific node (default: 1)"
        echo ""
        echo "Examples:"
        echo "  $0 start 3          Start 3-node testnet"
        echo "  $0 validate         Validate node consensus"
        echo "  $0 logs 2           View Node 2 logs"
        echo ""
        ;;
esac
