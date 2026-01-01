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
npm run test          # Run all tests (core + node)
npm run test:core     # Run core package tests
npm run test:node     # Run node package tests
```

### Testing
The project has comprehensive test coverage using vitest:
- **Core tests (66 tests)**: Crypto, encoding, merkle, DAG, weight, checkpoint
- **Node tests (22 tests)**: State management, mempool operations
- **Security tests**: Weight inflation attack prevention, forged proof rejection, validator authentication

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

## Smart Contracts (URL-Native)

### Overview
Rinku supports URL-native smart contracts where contract code and state are encoded in URLs, maintaining the self-crawlable property.

### Contract URL Format
```
/sc/{payload}
payload = base64url(deflate({
  type: "deploy",
  contractId: string,
  creator: fingerprint,
  wasmBase64: string,     // WASM bytecode (or mock for now)
  initState: {},
  tipUrls: [...],
  sig, ts
}))
```

### Contract Call (Embedded in Transaction)
```typescript
{
  ...transaction,
  contract: {
    action: "call",
    contractId: string,
    entrypoint: "mint" | "transfer" | "get_balance",
    input: { to: "...", amount: 100 },
    preStateHash: string,
    postStateHash: string
  }
}
```

### API Endpoints
- `GET /api/contracts` - List all deployed contracts
- `GET /api/contracts/:id` - Get contract details
- `GET /api/contracts/:id/state` - Get current state
- `GET /api/contracts/:id/history` - Get execution history
- `POST /api/contracts/deploy` - Deploy new contract
- `POST /api/contracts/:id/call` - Execute contract method
- `POST /api/contracts/:id/simulate` - Dry-run without state change

### Demo Script
```bash
cd packages/node
npm run demo-contract
```

### Supported Entrypoints (Mock Runtime)
- `init` - Initialize contract
- `mint` - Create tokens for an address
- `transfer` - Move tokens between addresses
- `get_balance` - Query balance (read-only)

### Future: Real WASM Execution
Currently uses a mock runtime that simulates token operations. Future versions will integrate:
- Deterministic WASM runtime (Wasmer/WasmTime)
- Gas metering with fuel limits
- Host bindings for ledger queries
- State Merkle commitments

## Rewards & Staking System

### Overview
Rinku implements a multi-mechanism reward system to incentivize network participation without traditional proof-of-work mining.

### Reward Types
1. **Tip Rewards** - Earned for validating orphaned transactions (referencing tips)
2. **Stake Rewards** - Earned by validators who stake tokens
3. **Witness Rewards** - Earned when your transactions are referenced by others

### Reward Configuration (Default)
```typescript
{
  tipRewardRate: 0.01,      // 1% of tx amount
  stakeRewardRate: 0.005,   // 0.5% per staked amount
  witnessRewardRate: 0.002, // 0.2% when witnessed
  minStakeAmount: 100,      // Minimum to become validator
  unstakeCooldownMs: 86400000 // 24 hour cooldown
}
```

### Staking API Endpoints
- `GET /api/staking` - Network staking overview
- `GET /api/staking/:address` - Individual staking status
- `POST /api/staking/stake` - Stake tokens
- `POST /api/staking/unstake` - Unstake tokens (after cooldown)

### Rewards API Endpoints
- `GET /api/rewards/config` - Reward configuration
- `GET /api/rewards/:address` - Rewards summary
- `POST /api/rewards/:address/claim` - Claim pending rewards

### Demo Script
```bash
cd packages/node
npm run demo-rewards
```

## Checkpoint & Finality Proofs

### Overview
Rinku implements checkpoint-based finality proofs that enable truly trustless verification from URLs alone. When validators create and sign checkpoints, finality proofs can be embedded in transaction URLs.

### How It Works
1. **Checkpoint Creation**: Periodic snapshots of network state (60s intervals)
2. **Validator Signatures**: Staked validators sign checkpoints with their keys
3. **Proof Embedding**: Finality proofs added as URL query parameters
4. **Standalone Verification**: Recipients verify signatures cryptographically without nodes

### Finalized URL Format
```
/tx/{payload}?proof={encodedProof}

proof = base64url({
  c: checkpointId,
  h: checkpointHeight,
  m: merkleRoot,
  n: signatureCount,
  w: totalValidatorWeight,
  s: [{ v: validator, g: signature, p: publicKey, w: weight, t: timestamp }]
})
```

### Verification Process (No Nodes Required)
1. Extract proof from URL query parameter
2. Recompute signing data from proof fields
3. Verify each signature against public key
4. Confirm validator fingerprint matches public key
5. Check weight threshold (51% of validator weight)

### Checkpoint API Endpoints
- `GET /api/checkpoints` - List all checkpoints
- `GET /api/checkpoints/:id` - Get specific checkpoint
- `POST /api/checkpoints/create` - Trigger new checkpoint
- `GET /api/tx/:hash/finalized` - Get transaction URL with proof

### Demo Script
```bash
cd packages/node
npm run demo-finality
```

### Finality Requirements
- Minimum 1 validator signature
- 51% of staked validator weight
- Valid cryptographic signatures

### Security Model (Trustless with Genesis Bootstrapping)
- **Genesis as root of trust**: `genesis_00000000` contains initial validator set and chain ID
- **Checkpoint chaining**: Each checkpoint has `previousCheckpointId` linking back to genesis
- **Validator authentication**: Proofs embed full validator set with addresses, public keys, and weights
- **Verification against trusted validators**: Weight thresholds computed from genesis/chain validators, NOT from proof
- **Weight inflation protection**: Attacker cannot forge proofs with inflated weights - denominator is computed from trusted set

**Attack Resistance:**
- Forged checkpoint signatures → Cryptographically rejected
- Inflated validator weights → Rejected (verified against genesis chain)
- Fake validators → Rejected (not in authenticated validator set)
- Lowered totalNetworkWeight → Rejected (recomputed from trusted validators)
- History rewriting → Rejected (checkpoint chain verified from genesis)

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
- **Smart Contracts**: URL-native contract system with mock WASM runtime
- **Contract API**: Deploy, call, simulate, and view contract state endpoints
- **Explorer Contracts Tab**: UI for deploying and interacting with contracts
- **Token Contract Demo**: Example script for testing contract operations
- **Rewards & Staking System**: Multi-mechanism rewards (tip, stake, witness)
- **Staking API**: Stake/unstake endpoints with cooldown periods
- **Rewards API**: Claim rewards, view summaries, check configuration
- **Explorer Rewards Tab**: UI for viewing rewards and staking
- **Checkpoint System**: Periodic checkpoints with validator signatures
- **Finality Proofs**: URL-embedded proofs for trustless verification
- **Cryptographic Verification**: Proof verification validates ed25519 signatures standalone
- **Demo Script**: `npm run demo-finality` shows URL-only finality verification
