#!/bin/bash
set -e

GENESIS_APP="rinku-genesis"
VALIDATOR1_APP="rinku-validator-1"
VALIDATOR2_APP="rinku-validator-2"
# VALIDATOR3_APP="rinku-validator-3"
# VALIDATOR4_APP="rinku-validator-4"
ALL_APPS=("$GENESIS_APP" "$VALIDATOR1_APP" "$VALIDATOR2_APP") # "$VALIDATOR3_APP" "$VALIDATOR4_APP"
VALIDATOR_APPS=("$VALIDATOR1_APP" "$VALIDATOR2_APP")
REGION="sjc"
CHAIN_ID="rinku-testnet"
NETWORK_ID="testnet"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

usage() {
    cat << EOF
Rinku Fly.io Deployment Script (5-node validator network)

Usage: $0 <mode> [options]

MODES:
  update              Redeploy all nodes with updated code (retains chain history)
  update-genesis      Redeploy only the genesis node (retains data)
  update-validators   Redeploy only validator nodes (retains data)
  fresh               Complete fresh deployment (wipes all data, restarts genesis)
  fresh-genesis       Fresh genesis only (wipes genesis data)
  status              Show status of all nodes
  bootstrap-info      Get bootstrap info from genesis node
  logs <app>          Show logs for specified app

OPTIONS:
  --skip-build        Skip Docker build (use existing image)
  --parallel          Deploy validators in parallel (faster but harder to debug)
  --genesis-only      Only deploy genesis node
  --help              Show this help message

NODES:
  Genesis:     ${GENESIS_APP}
  Validator 1: ${VALIDATOR1_APP}
  Validator 2: ${VALIDATOR2_APP}
  Validator 3: ${VALIDATOR3_APP}
  Validator 4: ${VALIDATOR4_APP}

EXAMPLES:
  $0 update                    # Update all nodes with new code, keep chain history
  $0 fresh                     # Wipe everything and start fresh
  $0 update-genesis            # Update only genesis node
  $0 status                    # Check status of all nodes
  $0 logs rinku-genesis        # View genesis node logs

EOF
    exit 0
}

check_fly_auth() {
    if ! fly auth whoami &>/dev/null; then
        log_error "Not authenticated with Fly.io. Run 'fly auth login' first."
        exit 1
    fi
    log_success "Authenticated with Fly.io"
}

app_exists() {
    fly apps list 2>/dev/null | grep -q "^$1 "
}

create_app_if_needed() {
    local app_name=$1
    if app_exists "$app_name"; then
        log_info "App $app_name already exists"
        return 0
    fi
    log_info "Creating app: $app_name"
    fly apps create "$app_name" --org personal
}

allocate_ipv4_if_needed() {
    local app_name=$1
    local has_ipv4=$(fly ips list -a "$app_name" 2>/dev/null | grep -c "v4" || echo "0")
    if [ "$has_ipv4" = "0" ]; then
        log_info "Allocating IPv4 for $app_name..."
        fly ips allocate-v4 -a "$app_name"
    else
        log_info "IPv4 already allocated for $app_name"
    fi
}

get_app_ipv4() {
    local app_name=$1
    local ip
    ip=$(fly ips list -a "$app_name" --json 2>/dev/null | grep -o '"Address":"[^"]*"' | head -1 | cut -d'"' -f4)
    if [ -z "$ip" ]; then
        ip=$(fly ips list -a "$app_name" 2>/dev/null | grep "v4" | head -1 | sed 's/│/|/g' | awk -F'|' '{for(i=1;i<=NF;i++){gsub(/^[ \t]+|[ \t]+$/,"",$i); if($i ~ /^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$/){print $i; exit}}}')
    fi
    echo "$ip"
}

destroy_all_machines() {
  local app_name="$1"
  log_warn "Destroying ALL machines for $app_name (keeping app/IPs)..."

  fly machine list -a "$app_name" -q 2>/dev/null | while IFS= read -r id; do
    id="$(echo "$id" | tr -d '[:space:]')"
    [ -n "$id" ] || continue

    log_info "Destroying machine: $id"
    fly machine destroy -a "$app_name" --force "$id" <<< "y" 2>/dev/null || true
  done
}

wait_for_no_machines() {
  local app_name="$1"
  local tries=30
  local i=0

  while [ $i -lt $tries ]; do
    local count
    count="$(fly machine list -a "$app_name" -q 2>/dev/null | wc -l | tr -d ' ')"
    if [ "$count" = "0" ]; then
      return 0
    fi
    i=$((i+1))
    sleep 2
  done

  log_warn "Machines still present for $app_name after waiting; continuing anyway."
  return 1
}

wipe_and_recreate_volume() {
    local app_name=$1
    log_warn "Wiping data volume for $app_name..."
    
    local max_attempts=3
    local attempt=0
    
    while [ $attempt -lt $max_attempts ]; do
        local volumes=$(fly volumes list -a "$app_name" 2>/dev/null | grep "rinku_data" | awk '{print $1}')
        
        if [ -z "$volumes" ]; then
            log_info "No volumes to destroy for $app_name"
            break
        fi
        
        for vol in $volumes; do
            log_info "Destroying volume: $vol"
            if fly volumes destroy "$vol" -a "$app_name" -y 2>&1; then
                log_success "Destroyed volume $vol"
            else
                log_warn "Failed to destroy volume $vol, will retry..."
            fi
        done
        
        sleep 3
        attempt=$((attempt + 1))
        
        local remaining=$(fly volumes list -a "$app_name" 2>/dev/null | grep -c "rinku_data" || echo "0")
        if [ "$remaining" = "0" ]; then
            log_success "All volumes destroyed for $app_name"
            break
        else
            log_warn "Still $remaining volumes remaining, attempt $attempt/$max_attempts"
        fi
    done
    
    log_info "Creating fresh volume for $app_name..."
    fly volumes create rinku_data -a "$app_name" --region "$REGION" --size 1 -y
    log_success "Volume created for $app_name"
}

deploy_app() {
    local app_name=$1
    local extra_args="${2:-}"
    
    log_info "Deploying $app_name..."
    
    fly deploy \
        --dockerfile Dockerfile.fly \
        --app "$app_name" \
        $extra_args
    
    log_success "Deployed $app_name"
}

get_bootstrap_info() {
    local genesis_url="https://${GENESIS_APP}.fly.dev"
    log_info "Fetching bootstrap info from genesis..."
    
    local max_attempts=30
    local attempt=0
    
    while [ $attempt -lt $max_attempts ]; do
        local response=$(curl -s "${genesis_url}/api/bootstrap" 2>/dev/null)
        if [ -n "$response" ] && echo "$response" | grep -q '"peerId"'; then
            echo "$response"
            return 0
        fi
        attempt=$((attempt + 1))
        log_info "Waiting for genesis to be ready... (attempt $attempt/$max_attempts)"
        sleep 10
    done
    
    log_error "Failed to get bootstrap info from genesis"
    return 1
}

get_bootstrap_info_for_app() {
    local app_name=$1
    local app_url="https://${app_name}.fly.dev"
    log_info "Fetching bootstrap info from ${app_name}..."
    
    local max_attempts=30
    local attempt=0
    
    while [ $attempt -lt $max_attempts ]; do
        local response=$(curl -s "${app_url}/api/bootstrap" 2>/dev/null)
        if [ -n "$response" ] && echo "$response" | grep -q '"peerId"'; then
            echo "$response"
            return 0
        fi
        attempt=$((attempt + 1))
        log_info "Waiting for ${app_name} to be ready... (attempt $attempt/$max_attempts)"
        sleep 10
    done
    
    log_error "Failed to get bootstrap info from ${app_name}"
    return 1
}

build_genesis_validators_env() {
    local validator_envs=()
    
    for app in "${ALL_APPS[@]}"; do
        local info
        info=$(get_bootstrap_info_for_app "$app") || return 1
        local val=$(echo "$info" | grep -o '"genesisValidatorEnv":"[^"]*"' | cut -d'"' -f4)
        if [ -z "$val" ]; then
            log_error "Failed to get validator env for $app"
            return 1
        fi
        validator_envs+=("$val")
    done
    
    local result=""
    for i in "${!validator_envs[@]}"; do
        if [ $i -gt 0 ]; then
            result="${result};"
        fi
        result="${result}${validator_envs[$i]}"
    done
    
    echo "$result"
}

apply_genesis_validators_secrets() {
    local genesis_validators_env=$1
    
    log_info "Applying GENESIS_VALIDATORS to all 5 nodes..."
    for app in "${ALL_APPS[@]}"; do
        fly secrets set -a "$app" GENESIS_VALIDATORS="$genesis_validators_env"
        fly secrets deploy -a "$app"
    done
    log_success "Applied GENESIS_VALIDATORS to all nodes"
}

configure_genesis() {
    local genesis_app=$1
    log_info "Configuring $genesis_app as genesis node..."
    
    local app_ip=$(get_app_ipv4 "$genesis_app")
    
    fly secrets set -a "$genesis_app" \
        IS_GENESIS_NODE="true" \
        MAINNET_MODE="true" \
        ALLOW_UNTRUSTED_GENESIS="true" \
        CHAIN_ID="$CHAIN_ID" \
        NETWORK_ID="$NETWORK_ID" \
        VALIDATOR_KEY_PASSWORD="testnet-${genesis_app}" \
        PUBLIC_URL="https://${genesis_app}.fly.dev"
    
    if [ -n "$app_ip" ]; then
        local p2p_external_addr="/ip4/${app_ip}/tcp/4001"
        log_info "Setting P2P_EXTERNAL_ADDR=${p2p_external_addr} for $genesis_app"
        fly secrets set -a "$genesis_app" P2P_EXTERNAL_ADDR="$p2p_external_addr"
    else
        log_warn "No IPv4 found for $genesis_app — PEX address sharing will be limited"
    fi
    
    log_success "Configured $genesis_app as genesis node"
}

configure_validator() {
    local validator_app=$1
    local genesis_ip=$2
    local peer_id=$3
    local genesis_validators_env=$4
    
    log_info "Configuring $validator_app as validator..."
    
    local bootstrap_peer="/ip4/${genesis_ip}/tcp/4001/p2p/${peer_id}"
    
    fly secrets set -a "$validator_app" \
        P2P_BOOTSTRAP_PEERS="$bootstrap_peer" \
        GENESIS_VALIDATORS="$genesis_validators_env" \
        IS_GENESIS_NODE="false" \
        MAINNET_MODE="true" \
        CHAIN_ID="$CHAIN_ID" \
        NETWORK_ID="$NETWORK_ID" \
        VALIDATOR_KEY_PASSWORD="testnet-${validator_app}" \
        PUBLIC_URL="https://${validator_app}.fly.dev"
    
    local app_ip=$(get_app_ipv4 "$validator_app")
    if [ -n "$app_ip" ]; then
        local p2p_external_addr="/ip4/${app_ip}/tcp/4001"
        log_info "Setting P2P_EXTERNAL_ADDR=${p2p_external_addr} for $validator_app"
        fly secrets set -a "$validator_app" P2P_EXTERNAL_ADDR="$p2p_external_addr"
    else
        log_warn "No IPv4 found for $validator_app — PEX address sharing will be limited"
    fi
    
    log_success "Configured $validator_app with bootstrap peer (IS_GENESIS_NODE=false)"
}

refresh_p2p_external_addrs() {
    log_info "Refreshing P2P_EXTERNAL_ADDR for all nodes..."
    for app in "${ALL_APPS[@]}"; do
        if app_exists "$app"; then
            local app_ip=$(get_app_ipv4 "$app")
            if [ -n "$app_ip" ]; then
                local p2p_external_addr="/ip4/${app_ip}/tcp/4001"
                log_info "Setting P2P_EXTERNAL_ADDR=${p2p_external_addr} for $app"
                fly secrets set -a "$app" P2P_EXTERNAL_ADDR="$p2p_external_addr"
            fi
        fi
    done
    log_success "P2P_EXTERNAL_ADDR refreshed"
}

show_status() {
    echo ""
    log_info "=== Rinku Network Status (5 validators) ==="
    echo ""
    
    for app in "${ALL_APPS[@]}"; do
        if app_exists "$app"; then
            echo -e "${GREEN}$app:${NC}"
            local status=$(fly status -a "$app" 2>/dev/null | grep -E "^(Machines|ID)" | head -5)
            echo "$status"
            local url="https://${app}.fly.dev"
            echo "  URL: $url"
            local health=$(curl -s "${url}/health" 2>/dev/null | head -c 100)
            if [ -n "$health" ]; then
                echo -e "  Health: ${GREEN}OK${NC}"
            else
                echo -e "  Health: ${RED}UNREACHABLE${NC}"
            fi
            echo ""
        else
            echo -e "${YELLOW}$app: NOT DEPLOYED${NC}"
            echo ""
        fi
    done
}

show_bootstrap_info() {
    if ! app_exists "$GENESIS_APP"; then
        log_error "Genesis app not deployed"
        exit 1
    fi
    
    log_info "Fetching bootstrap info..."
    local info=$(get_bootstrap_info)
    
    if [ -n "$info" ]; then
        echo ""
        echo -e "${GREEN}=== Bootstrap Info ===${NC}"
        echo "$info"
        echo ""
        
        local peer_id=$(echo "$info" | grep -o '"peerId":"[^"]*"' | cut -d'"' -f4)
        local genesis_ip=$(get_app_ipv4 "$GENESIS_APP")
        local genesis_val=$(echo "$info" | grep -o '"genesisValidatorEnv":"[^"]*"' | cut -d'"' -f4)
        local genesis_hash=$(echo "$info" | grep -o '"genesisHash":"[^"]*"' | cut -d'"' -f4)
        
        echo -e "${YELLOW}=== For Validator Configuration ===${NC}"
        echo "P2P_BOOTSTRAP_PEERS=/ip4/${genesis_ip}/tcp/4001/p2p/${peer_id}"
        echo "GENESIS_VALIDATORS=${genesis_val}"
        if [ -n "$genesis_hash" ]; then
            echo ""
            echo -e "${GREEN}=== Genesis Hash (Chain Identity) ===${NC}"
            echo "GENESIS_HASH=${genesis_hash}"
            echo ""
            echo "Nodes with different genesis hashes will reject sync from this network."
        fi
        echo ""
    fi
}

show_logs() {
    local app_name=$1
    if [ -z "$app_name" ]; then
        log_error "Please specify app name"
        exit 1
    fi
    fly logs -a "$app_name"
}

deploy_update() {
    log_info "=== Updating All 5 Nodes (Retaining Data) ==="
    
    check_fly_auth
    
    refresh_p2p_external_addrs
    
    log_info "Deploying genesis node..."
    deploy_app "$GENESIS_APP"
    
    sleep 10
    
    for app in "${VALIDATOR_APPS[@]}"; do
        if app_exists "$app"; then
            log_info "Deploying $app..."
            deploy_app "$app"
        fi
    done
    
    log_success "=== All nodes updated successfully ==="
    show_status
}

deploy_update_genesis() {
    log_info "=== Updating Genesis Node Only ==="
    check_fly_auth
    
    local app_ip=$(get_app_ipv4 "$GENESIS_APP")
    if [ -n "$app_ip" ]; then
        fly secrets set -a "$GENESIS_APP" P2P_EXTERNAL_ADDR="/ip4/${app_ip}/tcp/4001"
    fi
    
    deploy_app "$GENESIS_APP"
    log_success "Genesis node updated"
}

deploy_update_validators() {
    log_info "=== Updating All Validator Nodes ==="
    check_fly_auth
    
    refresh_p2p_external_addrs
    
    for app in "${VALIDATOR_APPS[@]}"; do
        if app_exists "$app"; then
            log_info "Deploying $app..."
            deploy_app "$app"
        fi
    done
    
    log_success "All validator nodes updated"
}

deploy_fresh() {
    log_info "=== Fresh Deployment — 5 Validator Nodes (Wiping All Data) ==="
    
    echo ""
    echo -e "${RED}WARNING: This will wipe all chain data and start fresh!${NC}"
    echo "This includes:"
    echo "  - Genesis node data"
    echo "  - Validator 1 data"  
    echo "  - Validator 2 data"
    echo "  - Validator 3 data"
    echo "  - Validator 4 data"
    echo ""
    read -p "Are you sure you want to continue? (yes/no): " confirm
    
    if [ "$confirm" != "yes" ]; then
        log_info "Aborted"
        exit 0
    fi
    
    check_fly_auth
    
    log_info "Step 1: Creating/preparing all 5 apps..."
    # for app in "${ALL_APPS[@]}"; do
    #     create_app_if_needed "$app"
    # done
    
    log_info "Step 2: Allocating IPv4 addresses..."
    for app in "${ALL_APPS[@]}"; do
        allocate_ipv4_if_needed "$app"
    done
    
    log_info "Step 3: Destroying existing machines (IPs are preserved)..."
    for app in "${ALL_APPS[@]}"; do
        if app_exists "$app"; then
            destroy_all_machines "$app"
            wait_for_no_machines "$app" || true
        fi
    done
    
    log_info "Step 4: Wiping volumes..."
    for app in "${ALL_APPS[@]}"; do
        wipe_and_recreate_volume "$app"
    done
    
    log_info "Step 5: Configuring and deploying genesis node..."
    configure_genesis "$GENESIS_APP"
    deploy_app "$GENESIS_APP"
    
    log_info "Step 6: Waiting for genesis to start..."
    sleep 30
    
    log_info "Step 7: Getting bootstrap info..."
    local bootstrap_info=$(get_bootstrap_info)
    
    if [ -z "$bootstrap_info" ]; then
        log_error "Failed to get bootstrap info"
        exit 1
    fi
    
    local peer_id=$(echo "$bootstrap_info" | grep -o '"peerId":"[^"]*"' | cut -d'"' -f4)
    local genesis_ip=$(get_app_ipv4 "$GENESIS_APP")
    local genesis_validator=$(echo "$bootstrap_info" | grep -o '"genesisValidatorEnv":"[^"]*"' | cut -d'"' -f4)
    
    log_info "Genesis Peer ID: $peer_id"
    log_info "Genesis IP: $genesis_ip"
    log_info "Genesis Validator: $genesis_validator"
    
    if [ -z "$genesis_ip" ] || ! echo "$genesis_ip" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$'; then
        log_error "Invalid or empty genesis IPv4 address: '${genesis_ip}'"
        log_error "Check 'fly ips list -a ${GENESIS_APP}' — you may need to allocate an IPv4."
        exit 1
    fi
    
    if [ -z "$peer_id" ]; then
        log_error "Failed to get genesis peer ID from bootstrap API"
        exit 1
    fi
    
    log_info "Step 8: Configuring all 4 validators (temporary GENESIS_VALIDATORS)..."
    for app in "${VALIDATOR_APPS[@]}"; do
        configure_validator "$app" "$genesis_ip" "$peer_id" "$genesis_validator"
    done
    
    log_info "Step 9: Deploying all 4 validators..."
    for app in "${VALIDATOR_APPS[@]}"; do
        deploy_app "$app"
    done

    log_info "Step 10: Building full GENESIS_VALIDATORS list from all 5 nodes..."
    local genesis_validators_env
    genesis_validators_env=$(build_genesis_validators_env)
    
    local validator_count=$(echo "$genesis_validators_env" | tr ';' '\n' | wc -l | tr -d ' ')
    # if [ "$validator_count" != "5" ]; then
    #     log_error "Expected 5 validators in GENESIS_VALIDATORS but got $validator_count"
    #     log_error "GENESIS_VALIDATORS: ${genesis_validators_env}"
    #     exit 1
    # fi
    log_info "GENESIS_VALIDATORS (${validator_count} validators): ${genesis_validators_env}"
    apply_genesis_validators_secrets "$genesis_validators_env"
    
    log_success "=== Fresh 5-node deployment complete! ==="
    echo ""
    show_status
    
    echo ""
    log_info "Network URLs:"
    echo "  Genesis:     https://${GENESIS_APP}.fly.dev"
    echo "  Validator 1: https://${VALIDATOR1_APP}.fly.dev"
    echo "  Validator 2: https://${VALIDATOR2_APP}.fly.dev"
    # echo "  Validator 3: https://${VALIDATOR3_APP}.fly.dev"
    # echo "  Validator 4: https://${VALIDATOR4_APP}.fly.dev"
    echo ""
    log_info "Explorer should connect to: https://${GENESIS_APP}.fly.dev"
    echo ""
    log_info "Monitor: npx tsx scripts/testnet-monitor.ts https://${GENESIS_APP}.fly.dev https://${VALIDATOR1_APP}.fly.dev https://${VALIDATOR2_APP}.fly.dev https://${VALIDATOR3_APP}.fly.dev https://${VALIDATOR4_APP}.fly.dev"
    echo ""
    log_info "Activity bot: RINKU_NODE_URLS=\"https://${GENESIS_APP}.fly.dev,https://${VALIDATOR1_APP}.fly.dev,https://${VALIDATOR2_APP}.fly.dev,https://${VALIDATOR3_APP}.fly.dev,https://${VALIDATOR4_APP}.fly.dev\" npx tsx scripts/activity-bot-v2.ts --mode=realistic --accounts=10 --duration=300"
}

deploy_fresh_genesis() {
    log_info "=== Fresh Genesis Deployment ==="
    
    echo ""
    echo -e "${RED}WARNING: This will wipe genesis node data!${NC}"
    echo "Validators will need to be reconfigured."
    echo ""
    read -p "Are you sure? (yes/no): " confirm
    
    if [ "$confirm" != "yes" ]; then
        log_info "Aborted"
        exit 0
    fi
    
    check_fly_auth
    
    create_app_if_needed "$GENESIS_APP"
    allocate_ipv4_if_needed "$GENESIS_APP"
    
    if app_exists "$GENESIS_APP"; then
        destroy_all_machines "$GENESIS_APP"
    fi
    
    wipe_and_recreate_volume "$GENESIS_APP"
    configure_genesis "$GENESIS_APP"
    deploy_app "$GENESIS_APP"
    
    log_success "Genesis deployed fresh!"
    
    sleep 20
    show_bootstrap_info
    
    log_warn "You need to reconfigure validators with the new bootstrap info above"
}

SKIP_BUILD=""
PARALLEL=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-build)
            SKIP_BUILD="--build-only"
            shift
            ;;
        --parallel)
            PARALLEL="true"
            shift
            ;;
        --help|-h)
            usage
            ;;
        update)
            deploy_update
            exit 0
            ;;
        update-genesis)
            deploy_update_genesis
            exit 0
            ;;
        update-validators)
            deploy_update_validators
            exit 0
            ;;
        fresh)
            deploy_fresh
            exit 0
            ;;
        fresh-genesis)
            deploy_fresh_genesis
            exit 0
            ;;
        status)
            show_status
            exit 0
            ;;
        bootstrap-info)
            show_bootstrap_info
            exit 0
            ;;
        logs)
            shift
            show_logs "$1"
            exit 0
            ;;
        *)
            log_error "Unknown command: $1"
            usage
            ;;
    esac
done

usage
