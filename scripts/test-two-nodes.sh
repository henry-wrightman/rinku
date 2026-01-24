#!/usr/bin/env bash
# Two-node local test script for Rinku
# This script starts a genesis node and a bootstrapping node to test sync
# MUST be run with bash: bash scripts/test-two-nodes.sh

set -e

# Get script directory (works in bash)
SCRIPT_DIR="$( cd "$( dirname "$0" )" && pwd )"
PROJECT_ROOT="$( cd "$SCRIPT_DIR/.." && pwd )"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${GREEN}=== Rinku Two-Node Test ===${NC}"

# Clean up old data
echo -e "${YELLOW}Cleaning up old data...${NC}"
rm -rf /tmp/rinku-node1 /tmp/rinku-node2

# Build the node
echo -e "${YELLOW}Building Rinku node...${NC}"
cd "$PROJECT_ROOT"
cargo build -p rinku-node 2>&1 | tail -10

# Find the binary - check common locations
NODE_BIN=""
for path in "$PROJECT_ROOT/target/debug/rinku-node" "./target/debug/rinku-node" "target/debug/rinku-node"; do
    if [ -f "$path" ]; then
        NODE_BIN="$path"
        break
    fi
done

if [ -z "$NODE_BIN" ]; then
    echo -e "${RED}Build failed - node binary not found${NC}"
    echo "Searched in:"
    echo "  - $PROJECT_ROOT/target/debug/rinku-node"
    echo "  - ./target/debug/rinku-node"
    ls -la target/debug/ 2>/dev/null | head -10 || echo "target/debug/ not found"
    exit 1
fi

echo -e "${GREEN}Found binary: $NODE_BIN${NC}"

echo -e "${GREEN}Build successful!${NC}"

# Use high ports to avoid conflicts with main workflow (3001) and other services
NODE1_PORT=4001
NODE2_PORT=4002
GOSSIP1_PORT=4011
GOSSIP2_PORT=4012

# Start Node 1 (Genesis node)
echo -e "${YELLOW}Starting Node 1 (Genesis) on port $NODE1_PORT...${NC}"
API_PORT=$NODE1_PORT \
GOSSIP_PORT=$GOSSIP1_PORT \
PUBLIC_URL="http://localhost:$NODE1_PORT" \
DATA_DIR="/tmp/rinku-node1" \
RUST_LOG="rinku_node=info,rinku_node::gossip=info" \
$NODE_BIN > /tmp/rinku-node1.log 2>&1 &
NODE1_PID=$!
echo "Node 1 PID: $NODE1_PID"

# Wait for Node 1 to start
sleep 5
if ! curl -s http://localhost:$NODE1_PORT/api/stats/network > /dev/null 2>&1; then
    echo -e "${RED}Node 1 failed to start!${NC}"
    cat /tmp/rinku-node1.log
    kill $NODE1_PID 2>/dev/null
    exit 1
fi
echo -e "${GREEN}Node 1 is running${NC}"

# Start Node 2 (Bootstrapping node) - connects to Node 1 via NODE_PEERS
echo -e "${YELLOW}Starting Node 2 (Bootstrapping) on port $NODE2_PORT...${NC}"
API_PORT=$NODE2_PORT \
GOSSIP_PORT=$GOSSIP2_PORT \
PUBLIC_URL="http://localhost:$NODE2_PORT" \
DATA_DIR="/tmp/rinku-node2" \
NODE_PEERS="http://localhost:$NODE1_PORT" \
RUST_LOG="rinku_node=info,rinku_node::gossip=info" \
$NODE_BIN > /tmp/rinku-node2.log 2>&1 &
NODE2_PID=$!
echo "Node 2 PID: $NODE2_PID"

# Wait for Node 2 to start
sleep 5
if ! curl -s http://localhost:$NODE2_PORT/api/stats/network > /dev/null 2>&1; then
    echo -e "${RED}Node 2 failed to start!${NC}"
    cat /tmp/rinku-node2.log
    kill $NODE1_PID $NODE2_PID 2>/dev/null
    exit 1
fi
echo -e "${GREEN}Node 2 is running${NC}"

# Give some time for initial sync
sleep 3

# Get initial state
echo -e "${YELLOW}Checking initial state...${NC}"
NODE1_TX=$(curl -s http://localhost:$NODE1_PORT/api/stats/network | jq -r '.totalTransactionsProcessed // 0')
NODE2_TX=$(curl -s http://localhost:$NODE2_PORT/api/stats/network | jq -r '.totalTransactionsProcessed // 0')
echo "Node 1 transactions: $NODE1_TX"
echo "Node 2 transactions: $NODE2_TX"

# Get faucet address from Node 1
FAUCET=$(curl -s http://localhost:$NODE1_PORT/api/accounts | jq -r '.accounts[0].fingerprint // empty')
echo "Faucet address: $FAUCET"

# Generate a test keypair (simple approach - use known test keypair)
TEST_ADDR="test123456789abcdef"
echo "Test address: $TEST_ADDR"

# Send a faucet request to Node 1
echo -e "${YELLOW}Sending faucet request to Node 1...${NC}"
FAUCET_RESPONSE=$(curl -s -X POST http://localhost:$NODE1_PORT/api/faucet/request \
    -H "Content-Type: application/json" \
    -d "{\"address\": \"$TEST_ADDR\"}")
echo "Faucet response: $FAUCET_RESPONSE"

TX_HASH=$(echo "$FAUCET_RESPONSE" | jq -r '.txHash // .hash // empty')
if [ -z "$TX_HASH" ]; then
    echo -e "${RED}Faucet request failed${NC}"
else
    echo -e "${GREEN}Transaction hash: $TX_HASH${NC}"
fi

# Wait for sync - gossip runs every 200ms, give it time to propagate
echo -e "${YELLOW}Waiting for sync (10 seconds)...${NC}"
sleep 10

# Check transaction counts again
NODE1_TX=$(curl -s http://localhost:$NODE1_PORT/api/stats/network | jq -r '.totalTransactionsProcessed // 0')
NODE2_TX=$(curl -s http://localhost:$NODE2_PORT/api/stats/network | jq -r '.totalTransactionsProcessed // 0')
echo "Node 1 transactions: $NODE1_TX"
echo "Node 2 transactions: $NODE2_TX"

# Check if transaction exists on both nodes
if [ -n "$TX_HASH" ]; then
    echo -e "${YELLOW}Checking transaction on both nodes...${NC}"
    TX1=$(curl -s "http://localhost:$NODE1_PORT/api/tx/$TX_HASH" | jq -r '.hash // empty')
    TX2=$(curl -s "http://localhost:$NODE2_PORT/api/tx/$TX_HASH" | jq -r '.hash // empty')
    
    if [ -n "$TX1" ]; then
        echo -e "${GREEN}Transaction found on Node 1${NC}"
    else
        echo -e "${RED}Transaction NOT found on Node 1${NC}"
    fi
    
    if [ -n "$TX2" ]; then
        echo -e "${GREEN}Transaction found on Node 2 (SYNCED!)${NC}"
    else
        echo -e "${RED}Transaction NOT found on Node 2${NC}"
    fi
fi

# Check account balance on both nodes
echo -e "${YELLOW}Checking test account on both nodes...${NC}"
BALANCE1=$(curl -s "http://localhost:$NODE1_PORT/api/account/$TEST_ADDR" | jq -r '.balance // 0')
BALANCE2=$(curl -s "http://localhost:$NODE2_PORT/api/account/$TEST_ADDR" | jq -r '.balance // 0')
echo "Balance on Node 1: $BALANCE1"
echo "Balance on Node 2: $BALANCE2"

if [ "$BALANCE1" == "$BALANCE2" ] && [ "$BALANCE1" != "0" ] && [ "$BALANCE1" != "null" ]; then
    echo -e "${GREEN}=== SUCCESS: Balances match! ===${NC}"
else
    echo -e "${RED}=== WARNING: Balances don't match or are zero ===${NC}"
fi

# Show recent logs
echo -e "${YELLOW}Recent Node 2 sync logs:${NC}"
grep -E "(sync|Sync|DAG|delta)" /tmp/rinku-node2.log | tail -20

# Cleanup function
cleanup() {
    echo -e "${YELLOW}Cleaning up...${NC}"
    kill $NODE1_PID $NODE2_PID 2>/dev/null || true
    echo -e "${GREEN}Done!${NC}"
}

trap cleanup EXIT

echo ""
echo -e "${GREEN}=== Test Complete ===${NC}"
echo "Node 1 logs: /tmp/rinku-node1.log"
echo "Node 2 logs: /tmp/rinku-node2.log"

# Clean up automatically
cleanup
