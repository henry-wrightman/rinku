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
NODE_BIN="$PROJECT_DIR/target/release/rinku-node"

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

# Disable mDNS in CI/local scripted runs — avoids docker bridge peer noise on GHA
export P2P_MDNS=false

kill_pid_file() {
    local pid_file=$1
    if [ -f "$pid_file" ]; then
        local pid
        pid=$(cat "$pid_file")
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            for _ in {1..10}; do
                kill -0 "$pid" 2>/dev/null || break
                sleep 0.5
            done
            kill -9 "$pid" 2>/dev/null || true
        fi
        rm -f "$pid_file"
    fi
}

cleanup() {
    log_info "Cleaning up..."
    kill_pid_file "$DATA_DIR_GENESIS/node.pid"
    kill_pid_file "$DATA_DIR_VAL1/node.pid"
    kill_pid_file "$DATA_DIR_VAL2/node.pid"
    # Fallback: kill any release rinku-node processes (CI has no other instances)
    pkill -f "$NODE_BIN" 2>/dev/null || true
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

# start_node name data_dir api_port p2p_port is_genesis mainnet_mode bootstrap_peers genesis_validators log_file
start_node() {
    local name=$1
    local data_dir=$2
    local api_port=$3
    local p2p_port=$4
    local is_genesis=$5
    local mainnet_mode=$6
    local bootstrap_peers=$7
    local genesis_validators=$8
    local log_file=$9
    local public_url="http://localhost:${api_port}"

    if [ ! -x "$NODE_BIN" ]; then
        log_error "Node binary not found at $NODE_BIN — run build_node first"
        exit 1
    fi

    log_info "Starting $name on API port $api_port (mainnet=$mainnet_mode)..."
    mkdir -p "$data_dir"

    RUST_LOG="rinku_node=info" \
    DATA_DIR="$data_dir" \
    API_PORT="$api_port" \
    P2P_PORT="$p2p_port" \
    IS_GENESIS_NODE="$is_genesis" \
    MAINNET_MODE="$mainnet_mode" \
    PUBLIC_URL="$public_url" \
    P2P_MDNS="false" \
    CHECKPOINT_INTERVAL_MS="5000" \
    VALIDATOR_KEY_PASSWORD="test-$name" \
    P2P_BOOTSTRAP_PEERS="$bootstrap_peers" \
    GENESIS_VALIDATORS="$genesis_validators" \
    "$NODE_BIN" >> "$log_file" 2>&1 &

    echo $! > "$data_dir/node.pid"
    log_info "$name PID: $(cat "$data_dir/node.pid")"
}

start_genesis() {
    local mainnet_mode="${1:-false}"
    local genesis_validators="${2:-}"
    start_node "genesis" "$DATA_DIR_GENESIS" "$GENESIS_API_PORT" "$GENESIS_P2P_PORT" \
        "true" "$mainnet_mode" "" "$genesis_validators" "$DATA_DIR_GENESIS/node.log"
}

start_validator() {
    local name=$1
    local data_dir=$2
    local api_port=$3
    local p2p_port=$4
    local bootstrap_peers=$5
    local genesis_validators=$6
    local mainnet_mode="${7:-false}"
    start_node "$name" "$data_dir" "$api_port" "$p2p_port" \
        "false" "$mainnet_mode" "$bootstrap_peers" "$genesis_validators" "$data_dir/node.log"
}

wait_for_genesis() {
    log_info "Waiting for genesis node..."
    for i in {1..30}; do
        if [ -f "$DATA_DIR_GENESIS/node.pid" ] && kill -0 "$(cat "$DATA_DIR_GENESIS/node.pid")" 2>/dev/null; then
            if curl -sf "http://localhost:$GENESIS_API_PORT/api/bootstrap" | grep -q "peerId"; then
                log_success "Genesis node is ready"
                return 0
            fi
        else
            log_error "Genesis process died during startup"
            tail -30 "$DATA_DIR_GENESIS/node.log" 2>/dev/null || true
            exit 1
        fi
        sleep 1
    done
    log_error "Genesis node failed to start"
    tail -30 "$DATA_DIR_GENESIS/node.log" 2>/dev/null || true
    exit 1
}

get_genesis_info() {
    local info
    info=$(curl -s "http://localhost:$GENESIS_API_PORT/api/bootstrap")
    GENESIS_PEER_ID=$(echo "$info" | grep -o '"peerId":"[^"]*"' | cut -d'"' -f4)
    GENESIS_VALIDATOR=$(echo "$info" | grep -o '"genesisValidatorEnv":"[^"]*"' | cut -d'"' -f4)
    log_info "Genesis Peer ID: $GENESIS_PEER_ID"
    log_info "Genesis Validator: ${GENESIS_VALIDATOR:0:40}..."
}

wait_for_validators() {
    log_info "Waiting for validators to start..."
    for i in {1..30}; do
        local v1_alive=0 v2_alive=0 v1_ready=0 v2_ready=0
        if [ -f "$DATA_DIR_VAL1/node.pid" ] && kill -0 "$(cat "$DATA_DIR_VAL1/node.pid")" 2>/dev/null; then
            v1_alive=1
        fi
        if [ -f "$DATA_DIR_VAL2/node.pid" ] && kill -0 "$(cat "$DATA_DIR_VAL2/node.pid")" 2>/dev/null; then
            v2_alive=1
        fi
        if curl -sf "http://localhost:$VAL1_API_PORT/health" 2>/dev/null | grep -q '"status":"ok"'; then
            v1_ready=1
        fi
        if curl -sf "http://localhost:$VAL2_API_PORT/health" 2>/dev/null | grep -q '"status":"ok"'; then
            v2_ready=1
        fi
        if [ "$v1_alive" = "1" ] && [ "$v2_alive" = "1" ] && [ "$v1_ready" = "1" ] && [ "$v2_ready" = "1" ]; then
            log_success "All validators are ready"
            return 0
        fi
        sleep 1
    done
    log_error "Validators failed to start"
    tail -20 "$DATA_DIR_VAL1/node.log" 2>/dev/null || true
    tail -20 "$DATA_DIR_VAL2/node.log" 2>/dev/null || true
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

    log_info "Starting genesis with full validator list..."
    start_genesis "true" "$FULL_GENESIS_VALIDATORS"

    sleep 5
    wait_for_genesis

    get_genesis_info
    BOOTSTRAP_PEERS="/ip4/127.0.0.1/tcp/$GENESIS_P2P_PORT/p2p/$GENESIS_PEER_ID"

    start_validator "validator-1" "$DATA_DIR_VAL1" "$VAL1_API_PORT" "$VAL1_P2P_PORT" \
        "$BOOTSTRAP_PEERS" "$FULL_GENESIS_VALIDATORS" "true"
    start_validator "validator-2" "$DATA_DIR_VAL2" "$VAL2_API_PORT" "$VAL2_P2P_PORT" \
        "$BOOTSTRAP_PEERS" "$FULL_GENESIS_VALIDATORS" "true"

    wait_for_validators
}

get_validator_count() {
    local port=$1
    curl -sf "http://localhost:${port}/api/network/stats" 2>/dev/null \
        | grep -o '"validatorCount":[0-9]*' | cut -d':' -f2
}

get_checkpoint_count() {
    local port=$1
    curl -sf "http://localhost:${port}/api/network/stats" 2>/dev/null \
        | grep -o '"checkpointCount":[0-9]*' | cut -d':' -f2
}

generate_test_address() {
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -hex 20
    else
        printf '%040x' "${RANDOM}${RANDOM}${RANDOM}${RANDOM}"
    fi
}

# Leaders skip checkpoint creation when the mempool is empty ("network idle").
submit_test_transactions() {
    local count="${1:-24}"
    local nodes=(
        "http://localhost:$GENESIS_API_PORT"
        "http://localhost:$VAL1_API_PORT"
        "http://localhost:$VAL2_API_PORT"
    )
    local submitted=0
    local i node addr

    log_info "Submitting up to $count faucet transactions..."
    for ((i=0; i<count; i++)); do
        node="${nodes[$((i % 3))]}"
        addr=$(generate_test_address)
        if curl -sf -X POST "${node}/api/faucet/request" \
            -H "Content-Type: application/json" \
            -d "{\"address\":\"${addr}\"}" >/dev/null 2>&1; then
            submitted=$((submitted + 1))
        fi
        sleep 0.05
    done
    log_info "Submitted $submitted/$count faucet transactions"
}

check_validator_convergence() {
    log_info "Checking validator convergence..."
    sleep 5

    local g_validators v1_validators v2_validators
    g_validators=$(get_validator_count "$GENESIS_API_PORT")
    v1_validators=$(get_validator_count "$VAL1_API_PORT")
    v2_validators=$(get_validator_count "$VAL2_API_PORT")

    echo ""
    log_info "=== Validator Counts ==="
    echo "  Genesis:     ${g_validators:-?} validators"
    echo "  Validator-1: ${v1_validators:-?} validators"
    echo "  Validator-2: ${v2_validators:-?} validators"

    if [ "$g_validators" = "$v1_validators" ] && [ "$v1_validators" = "$v2_validators" ] && [ "$g_validators" = "3" ]; then
        log_success "All nodes see exactly 3 validators - CONVERGENCE OK!"
        return 0
    else
        log_error "Validator counts don't match or not equal to 3"
        return 1
    fi
}

wait_for_checkpoints() {
    log_info "Waiting for checkpoint creation (up to 3 minutes)..."
    log_info "Note: nodes skip empty checkpoints — submitting test transactions first"
    submit_test_transactions 30

    for i in {1..36}; do
        local g_cp v1_cp v2_cp
        g_cp=$(get_checkpoint_count "$GENESIS_API_PORT")
        v1_cp=$(get_checkpoint_count "$VAL1_API_PORT")
        v2_cp=$(get_checkpoint_count "$VAL2_API_PORT")

        echo "  Checkpoints: Genesis=${g_cp:-0}, V1=${v1_cp:-0}, V2=${v2_cp:-0}"

        if [ "${g_cp:-0}" -ge 3 ] && [ "${v1_cp:-0}" -ge 3 ] && [ "${v2_cp:-0}" -ge 3 ]; then
            log_success "All nodes have reached checkpoint height 3 - LEADER ELECTION WORKING!"
            return 0
        fi

        if [ $((i % 6)) -eq 0 ]; then
            submit_test_transactions 12
        fi
        sleep 5
    done

    log_error "Nodes failed to create checkpoints"
    log_info "Recent checkpoint activity from genesis:"
    grep -iE "checkpoint|leader election|network idle|skipping" "$DATA_DIR_GENESIS/node.log" 2>/dev/null | tail -15 || true
    return 1
}

show_logs() {
    log_info "Recent leader election logs from genesis:"
    grep -i "leader election" "$DATA_DIR_GENESIS/node.log" 2>/dev/null | tail -10 || echo "No leader election logs yet"

    echo ""
    log_info "Recent leader election logs from validator-1:"
    grep -i "leader election" "$DATA_DIR_VAL1/node.log" 2>/dev/null | tail -10 || echo "No leader election logs yet"
}

CI_MODE=false

run_multi_node_validation() {
    log_info "=== Phase 5: Multi-node validation ==="
    cd "$PROJECT_DIR"
    npx tsx scripts/validate-multi-node.ts \
        "http://localhost:$GENESIS_API_PORT" \
        "http://localhost:$VAL1_API_PORT" \
        "http://localhost:$VAL2_API_PORT"
}

main() {
    echo ""
    echo "=============================================="
    echo "  Rinku Local 3-Node Validator Test"
    echo "=============================================="
    echo ""

    if [ "$1" = "--clean" ]; then
        cleanup
        cleanup_data
        log_success "All test data cleaned"
        exit 0
    fi

    if [ "$1" = "--ci" ]; then
        CI_MODE=true
        shift
        trap 'cleanup; cleanup_data' EXIT
    fi

    cleanup
    cleanup_data

    build_node

    log_info "=== Phase 1: Initial deployment ==="
    start_genesis "false"
    wait_for_genesis
    get_genesis_info

    BOOTSTRAP_PEERS="/ip4/127.0.0.1/tcp/$GENESIS_P2P_PORT/p2p/$GENESIS_PEER_ID"

    start_validator "validator-1" "$DATA_DIR_VAL1" "$VAL1_API_PORT" "$VAL1_P2P_PORT" \
        "$BOOTSTRAP_PEERS" "$GENESIS_VALIDATOR" "false"
    start_validator "validator-2" "$DATA_DIR_VAL2" "$VAL2_API_PORT" "$VAL2_P2P_PORT" \
        "$BOOTSTRAP_PEERS" "$GENESIS_VALIDATOR" "false"

    wait_for_validators

    get_all_validator_info

    log_info "=== Phase 2: Restart with full GENESIS_VALIDATORS ==="
    restart_all_with_full_validators

    log_info "=== Phase 3: Verify convergence ==="
    check_validator_convergence

    log_info "=== Phase 4: Wait for checkpoints ==="
    wait_for_checkpoints

    show_logs

    if [ "$CI_MODE" = true ]; then
        run_multi_node_validation
        log_success "=== CI INTEGRATION TEST COMPLETE ==="
        exit 0
    fi

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
