# Rinku - URL-Native Distributed Ledger

## Overview
Rinku (Japanese for "link") is a URL-native distributed ledger with DAG-based consensus and weight-based Sybil resistance. The entire state exists as cryptographically-linked URLs.

## Project Structure
```
rinku/
├── packages/
│   ├── core/       # Shared library (types, crypto, encoding, merkle, dag, weight)
│   ├── wallet/     # Client wallet library (key management, transaction creation)
│   ├── node/       # Validator/Node (mempool, consensus, state, API)
│   ├── faucet/     # Testnet faucet for distributing coins
│   └── explorer/   # React-based block explorer
├── package.json    # Workspace root
└── tsconfig.base.json
```

## Key Concepts

### DAG-Based Ledger
- Each account maintains its own micro-chain of transactions
- Transactions reference 2+ prior "tips" from other accounts
- Conflicts resolved by cumulative weight
- No single coordinator - consensus emerges from weighted votes

### Weight Calculation (Sybil Resistance)
```
weight = (account_age_days * 0.3) + (balance * 0.7)
```

### Transaction URL Format
```
/tx/{payload}
payload = base64url(deflate({
  from: fingerprint,
  to: fingerprint,
  amount: number,
  nonce: number,
  tipUrls: ["/tx/...", "/tx/..."],  # Full parent URLs - self-crawlable!
  sig: signature,
  ts: timestamp
}))
```

### Self-Crawlable Ledger
Each transaction embeds the **full URLs** of its parent transactions. This means:
- Anyone with a single tip URL can reconstruct the entire ledger
- No node infrastructure needed - just the URLs themselves
- Complete cryptographic validation from any transaction URL

## Running the Project

### Development
- **Explorer**: Runs on port 5000 (main frontend)
- **Node API**: Runs on port 3001
- **Faucet**: Runs on port 3002

### Commands
```bash
npm run dev:explorer  # Start explorer frontend
npm run dev:node      # Start node server
npm run dev:faucet    # Start faucet server
```

## Technology Stack
- TypeScript with npm workspaces
- React + Vite for explorer frontend
- Express for API servers
- Web Crypto API for cryptography
- pako for DEFLATE compression

## Multi-Node Networking

### Environment Variables
- `NODE_PORT`: Port for node API (default: 3001)
- `NODE_ID`: Unique node identifier (auto-generated if not set)
- `NODE_PEERS`: Comma-separated list of peer URLs (e.g., "http://peer1:3001,http://peer2:3001")
- `RINKU_DATA_DIR`: Directory for persistence (default: .rinku-data)

### Persistence
State and DAG are persisted to JSON files in the data directory. On restart, nodes restore from the snapshot automatically.

### Peer Sync Protocol
- `GET /api/sync/status`: Node status (merkleRoot, dagSize, tips)
- `GET /api/sync/transactions`: All transactions with public keys
- `GET /api/sync/peers`: List of configured peers
- `POST /api/sync/force`: Force sync with all peers

### Running Multiple Nodes
```bash
# Node 1 (Replit)
NODE_PORT=3001 NODE_ID=node1 npm run dev:node

# Node 2 (local, syncs from Node 1)
NODE_PORT=3002 NODE_ID=node2 NODE_PEERS=https://your-replit-url.repl.co npm run dev:node
```

### Wallet CLI Environment Variables
- `RINKU_NODE_URL`: Node API URL (default: http://localhost:3001)
- `RINKU_FAUCET_URL`: Faucet API URL (default: http://localhost:3002)

### Network Simulation
```bash
# Generate 100 wallets and validate the entire ledger from a single tip URL
cd packages/node
WALLET_COUNT=100 npm run simulate

# Results show:
# - All transactions crawled from a single URL
# - Complete account balance reconstruction
# - Chain depth and linking structure
```

### Stress Testing
```bash
# Generate 500 faucet transactions
cd packages/node
TX_COUNT=500 npm run stress-test

# Test bootstrap on another machine
rm -rf .rinku-data
NODE_PORT=3003 NODE_PEERS=https://your-replit-url npm run dev:node
```

## Testnet Deployment Strategy

### Recommended Domain Structure
```
explorer.testnet.rinku.xyz  → Port 5000 (Frontend)
node.testnet.rinku.xyz      → Port 3001 (Node API)
faucet.testnet.rinku.xyz    → Port 3002 (Faucet API)

# Future multi-node:
node-1.testnet.rinku.xyz
node-2.testnet.rinku.xyz
```

### Deployment Type
- **Use VM deployment** (not autoscale) - nodes need persistent storage and always-on uptime
- Autoscale instances sleep between requests, causing ledger drift and broken peer sync

### Multi-Node Setup
1. Deploy first node as canonical source
2. Additional nodes set `NODE_PEERS=https://node.testnet.rinku.xyz`
3. Use `/api/sync/peers` endpoint for peer discovery
4. Consider a git-tracked peers manifest for production

### Steps to Deploy
1. Configure VM deployment with environment variables (NODE_PORT, NODE_ID, NODE_PEERS)
2. Purchase domain and create DNS records for subdomains
3. Set up TLS certificates (wildcard cert recommended for *.testnet.rinku.xyz)
4. Configure reverse proxy to route subdomains to correct ports

## Recent Changes
- Initial project setup with all 5 packages
- Core library with types, crypto, encoding, merkle, dag, weight modules
- Node server with mempool, consensus, state management, and REST API
- Wallet library for key management and transaction creation
- Faucet for testnet coin distribution
- Explorer with DAG visualization, accounts view, and faucet integration
- Added persistence layer for state/DAG snapshots
- Added peer sync service for multi-node networking
- Added sync API endpoints for node-to-node communication
- Fixed cold-start bootstrap (sync from peers before creating genesis)
- Added stress test script for load testing (npm run stress-test)
- Added configurable env vars for wallet CLI (RINKU_NODE_URL, RINKU_FAUCET_URL)
- **URL-Native Transactions**: Changed from hash-based tips to URL-based tipUrls
- **Self-Crawlable Ledger**: Entire ledger can be reconstructed from any single tip URL
- **Stateless Validator**: Added @rinku/stateless package for validating from URLs
- **Network Simulation**: Added `npm run simulate` to generate and validate large networks
