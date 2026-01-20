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
BASE_PORT=3001
NODE_PORTS=()
NUM_NODES=${2:-2}
LOG_DIR="$PROJECT_ROOT/.testnet-logs"

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
    
    # Start each node
    for ((i=0; i<NUM_NODES; i++)); do
        local port=${NODE_PORTS[$i]}
        local peers=$(build_peer_list $i)
        local log_file="$LOG_DIR/node-$((i+1)).log"
        local db_path="$LOG_DIR/node-$((i+1))-data"
        
        log_info "Starting Node $((i+1)) on port $port..."
        
        # Clean up old database for fresh start (optional)
        # rm -rf "$db_path"
        
        # Set environment variables and start node (using debug binary directly)
        # Use DATA_DIR, PORT, and P2P_PORT env vars that the node actually reads
        local p2p_port=$((4001 + i))
        
        PORT=$port \
        P2P_PORT=$p2p_port \
        NODE_PEERS="$peers" \
        DATA_DIR="$db_path" \
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
        
        # Wait a bit between starting nodes to avoid port conflicts
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
    calculate_ports
    
    # Build URL list
    local urls=""
    for ((i=0; i<NUM_NODES; i++)); do
        if [ -n "$urls" ]; then
            urls="$urls "
        fi
        urls="${urls}http://localhost:${NODE_PORTS[$i]}"
    done
    
    if [ -z "$urls" ]; then
        # Default to checking for running nodes
        for pid_file in "$LOG_DIR"/node-*.pid; do
            if [ -f "$pid_file" ]; then
                local node_num=$(basename "$pid_file" .pid | sed 's/node-//')
                local port=$((BASE_PORT + node_num - 1))
                if [ -n "$urls" ]; then
                    urls="$urls "
                fi
                urls="${urls}http://localhost:$port"
            fi
        done
    fi
    
    if [ -z "$urls" ]; then
        log_error "No nodes to validate. Start testnet first."
        exit 1
    fi
    
    log_info "Running multi-node validation..."
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
