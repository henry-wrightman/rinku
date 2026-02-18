#!/bin/bash
# Local 3-Node Validator Test Script
# Tests the validator registry and leader election convergence locally

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Data directories
DATA_DIR_GENESIS="$PROJECT_DIR/.test-genesis"
DATA_DIR_VAL1="$PROJECT_DIR/.test-validator-1"
DATA_DIR_VAL2="$PROJECT_DIR/.test-validator-2"

# Ports
GENESIS_API_PORT=3001
VAL1_API_PORT=3002
VAL2_API_PORT=3003
GENESIS_P2P_PORT=4001
VAL1_P2P_PORT=4002
VAL2_P2P_PORT=4003

cleanup() {
    log_info "Cleaning up..."
    pkill -f "rinku-node.*test-genesis" 2>/dev/null || true
    pkill -f "rinku-node.*test-validator-1" 2>/dev/null || true
    pkill -f "rinku-node.*test-validator-2" 2>/dev/null || true
    sleep 2
}

cleanup_data() {
    log_warn "Wiping all test data..."
    rm -rf "$DATA_DIR_GENESIS" "$DATA_DIR_VAL1" "$DATA_DIR_VAL2"
}

build_node() {
    log_info "Building node..."
    cd "$PROJECT_DIR"
    cargo build -p rinku-node --release 2>&1 | tail -5
    log_success "Node built"
}

start_genesis() {
    log_info "Starting genesis node on port $GENESIS_API_PORT..."
    mkdir -p "$DATA_DIR_GENESIS"
    
    RUST_LOG="rinku_node=info" \
    DATA_DIR="$DATA_DIR_GENESIS" \
    API_PORT="$GENESIS_API_PORT" \
    P2P_PORT="$GENESIS_P2P_PORT" \
    IS_GENESIS_NODE="true" \
    MAINNET_MODE="false" \
    CHECKPOINT_INTERVAL_MS="10000" \
    VALIDATOR_KEY_PASSWORD="test-genesis" \
    "$PROJECT_DIR/target/release/rinku-node" 2>&1 | tee "$DATA_DIR_GENESIS/node.log" &
    
    echo $! > "$DATA_DIR_GENESIS/node.pid"
    log_info "Genesis PID: $(cat $DATA_DIR_GENESIS/node.pid)"
}

wait_for_genesis() {
    log_info "Waiting for genesis node..."
    for i in {1..30}; do
        if curl -s "http://localhost:$GENESIS_API_PORT/api/bootstrap" | grep -q "peerId"; then
            log_success "Genesis node is ready"
            return 0
        fi
        sleep 1
    done
    log_error "Genesis node failed to start"
    exit 1
}

get_genesis_info() {
    local info=$(curl -s "http://localhost:$GENESIS_API_PORT/api/bootstrap")
    GENESIS_PEER_ID=$(echo "$info" | grep -o '"peerId":"[^"]*"' | cut -d'"' -f4)
    GENESIS_VALIDATOR=$(echo "$info" | grep -o '"genesisValidatorEnv":"[^"]*"' | cut -d'"' -f4)
    log_info "Genesis Peer ID: $GENESIS_PEER_ID"
    log_info "Genesis Validator: ${GENESIS_VALIDATOR:0:40}..."
}

start_validator() {
    local name=$1
    local data_dir=$2
    local api_port=$3
    local p2p_port=$4
    local bootstrap_peers=$5
    local genesis_validators=$6
    
    log_info "Starting $name on port $api_port..."
    mkdir -p "$data_dir"
    
    RUST_LOG="rinku_node=info" \
    DATA_DIR="$data_dir" \
    API_PORT="$api_port" \
    P2P_PORT="$p2p_port" \
    IS_GENESIS_NODE="false" \
    MAINNET_MODE="false" \
    CHECKPOINT_INTERVAL_MS="10000" \
    VALIDATOR_KEY_PASSWORD="test-$name" \
    P2P_BOOTSTRAP_PEERS="$bootstrap_peers" \
    GENESIS_VALIDATORS="$genesis_validators" \
    "$PROJECT_DIR/target/release/rinku-node" 2>&1 | tee "$data_dir/node.log" &
    
    echo $! > "$data_dir/node.pid"
    log_info "$name PID: $(cat $data_dir/node.pid)"
}

wait_for_validators() {
    log_info "Waiting for validators to start..."
    for i in {1..30}; do
        local v1_ready=$(curl -s "http://localhost:$VAL1_API_PORT/health" 2>/dev/null | grep -c "ok" || echo 0)
        local v2_ready=$(curl -s "http://localhost:$VAL2_API_PORT/health" 2>/dev/null | grep -c "ok" || echo 0)
        if [ "$v1_ready" = "1" ] && [ "$v2_ready" = "1" ]; then
            log_success "All validators are ready"
            return 0
        fi
        sleep 1
    done
    log_error "Validators failed to start"
    exit 1
}

get_all_validator_info() {
    log_info "Getting all validator info..."
    
    VAL1_VALIDATOR=$(curl -s "http://localhost:$VAL1_API_PORT/api/bootstrap" | grep -o '"genesisValidatorEnv":"[^"]*"' | cut -d'"' -f4)
    VAL2_VALIDATOR=$(curl -s "http://localhost:$VAL2_API_PORT/api/bootstrap" | grep -o '"genesisValidatorEnv":"[^"]*"' | cut -d'"' -f4)
    
    FULL_GENESIS_VALIDATORS="${GENESIS_VALIDATOR};${VAL1_VALIDATOR};${VAL2_VALIDATOR}"
    log_info "Full GENESIS_VALIDATORS: ${FULL_GENESIS_VALIDATORS:0:80}..."
}

restart_all_with_full_validators() {
    log_info "Restarting all nodes with full GENESIS_VALIDATORS..."
    
    cleanup
    sleep 2
    
    BOOTSTRAP_PEERS="/ip4/127.0.0.1/tcp/$GENESIS_P2P_PORT/p2p/$GENESIS_PEER_ID"
    
    # Start genesis with full validator list
    log_info "Starting genesis with full validator list..."
    RUST_LOG="rinku_node=info" \
    DATA_DIR="$DATA_DIR_GENESIS" \
    API_PORT="$GENESIS_API_PORT" \
    P2P_PORT="$GENESIS_P2P_PORT" \
    IS_GENESIS_NODE="true" \
    MAINNET_MODE="true" \
    CHECKPOINT_INTERVAL_MS="10000" \
    VALIDATOR_KEY_PASSWORD="test-genesis" \
    GENESIS_VALIDATORS="$FULL_GENESIS_VALIDATORS" \
    "$PROJECT_DIR/target/release/rinku-node" 2>&1 | tee "$DATA_DIR_GENESIS/node2.log" &
    echo $! > "$DATA_DIR_GENESIS/node.pid"
    
    sleep 5
    wait_for_genesis
    
    # Update genesis info after restart
    get_genesis_info
    BOOTSTRAP_PEERS="/ip4/127.0.0.1/tcp/$GENESIS_P2P_PORT/p2p/$GENESIS_PEER_ID"
    
    # Start validators with full list
    start_validator "validator-1" "$DATA_DIR_VAL1" "$VAL1_API_PORT" "$VAL1_P2P_PORT" "$BOOTSTRAP_PEERS" "$FULL_GENESIS_VALIDATORS"
    start_validator "validator-2" "$DATA_DIR_VAL2" "$VAL2_API_PORT" "$VAL2_P2P_PORT" "$BOOTSTRAP_PEERS" "$FULL_GENESIS_VALIDATORS"
    
    wait_for_validators
}

check_validator_convergence() {
    log_info "Checking validator convergence..."
    sleep 5
    
    # Check how many validators each node sees
    local g_validators=$(curl -s "http://localhost:$GENESIS_API_PORT/api/stats" | grep -o '"validatorCount":[0-9]*' | cut -d':' -f2)
    local v1_validators=$(curl -s "http://localhost:$VAL1_API_PORT/api/stats" | grep -o '"validatorCount":[0-9]*' | cut -d':' -f2)
    local v2_validators=$(curl -s "http://localhost:$VAL2_API_PORT/api/stats" | grep -o '"validatorCount":[0-9]*' | cut -d':' -f2)
    
    echo ""
    log_info "=== Validator Counts ==="
    echo "  Genesis:     $g_validators validators"
    echo "  Validator-1: $v1_validators validators"
    echo "  Validator-2: $v2_validators validators"
    
    if [ "$g_validators" = "$v1_validators" ] && [ "$v1_validators" = "$v2_validators" ] && [ "$g_validators" = "3" ]; then
        log_success "All nodes see exactly 3 validators - CONVERGENCE OK!"
        return 0
    else
        log_error "Validator counts don't match or not equal to 3"
        return 1
    fi
}

wait_for_checkpoints() {
    log_info "Waiting for checkpoint creation (up to 2 minutes)..."
    
    for i in {1..24}; do
        local g_cp=$(curl -s "http://localhost:$GENESIS_API_PORT/api/stats" | grep -o '"checkpointCount":[0-9]*' | cut -d':' -f2)
        local v1_cp=$(curl -s "http://localhost:$VAL1_API_PORT/api/stats" | grep -o '"checkpointCount":[0-9]*' | cut -d':' -f2)
        local v2_cp=$(curl -s "http://localhost:$VAL2_API_PORT/api/stats" | grep -o '"checkpointCount":[0-9]*' | cut -d':' -f2)
        
        echo "  Checkpoints: Genesis=$g_cp, V1=$v1_cp, V2=$v2_cp"
        
        if [ "$g_cp" -ge "3" ] && [ "$v1_cp" -ge "3" ] && [ "$v2_cp" -ge "3" ]; then
            log_success "All nodes have reached checkpoint 3 - LEADER ELECTION WORKING!"
            return 0
        fi
        sleep 5
    done
    
    log_error "Nodes failed to create checkpoints"
    return 1
}

show_logs() {
    log_info "Recent leader election logs from genesis:"
    grep -i "leader election" "$DATA_DIR_GENESIS/node2.log" 2>/dev/null | tail -10 || echo "No leader election logs yet"
    
    echo ""
    log_info "Recent leader election logs from validator-1:"
    grep -i "leader election" "$DATA_DIR_VAL1/node.log" 2>/dev/null | tail -10 || echo "No leader election logs yet"
}

main() {
    echo ""
    echo "=============================================="
    echo "  Rinku Local 3-Node Validator Test"
    echo "=============================================="
    echo ""
    
    # Parse args
    if [ "$1" = "--clean" ]; then
        cleanup
        cleanup_data
        log_success "All test data cleaned"
        exit 0
    fi
    
    cleanup
    cleanup_data
    
    build_node
    
    # Phase 1: Start nodes without full validator list (simulates initial deploy)
    log_info "=== Phase 1: Initial deployment ==="
    start_genesis
    wait_for_genesis
    get_genesis_info
    
    BOOTSTRAP_PEERS="/ip4/127.0.0.1/tcp/$GENESIS_P2P_PORT/p2p/$GENESIS_PEER_ID"
    
    start_validator "validator-1" "$DATA_DIR_VAL1" "$VAL1_API_PORT" "$VAL1_P2P_PORT" "$BOOTSTRAP_PEERS" "$GENESIS_VALIDATOR"
    start_validator "validator-2" "$DATA_DIR_VAL2" "$VAL2_API_PORT" "$VAL2_P2P_PORT" "$BOOTSTRAP_PEERS" "$GENESIS_VALIDATOR"
    
    wait_for_validators
    
    # Get full validator list
    get_all_validator_info
    
    # Phase 2: Restart with full validator list (simulates secrets update)
    log_info "=== Phase 2: Restart with full GENESIS_VALIDATORS ==="
    restart_all_with_full_validators
    
    # Phase 3: Verify convergence
    log_info "=== Phase 3: Verify convergence ==="
    check_validator_convergence
    
    # Phase 4: Wait for checkpoint creation
    log_info "=== Phase 4: Wait for checkpoints ==="
    wait_for_checkpoints
    
    # Show logs
    show_logs
    
    echo ""
    log_success "=== TEST COMPLETE ==="
    echo ""
    echo "Nodes are still running. Use './scripts/test-local-3-nodes.sh --clean' to stop and clean up."
    echo ""
    echo "API endpoints:"
    echo "  Genesis:     http://localhost:$GENESIS_API_PORT"
    echo "  Validator-1: http://localhost:$VAL1_API_PORT"
    echo "  Validator-2: http://localhost:$VAL2_API_PORT"
}

main "$@"
