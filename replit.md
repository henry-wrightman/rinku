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
  tips: [tx_hash, tx_hash],
  sig: signature,
  ts: timestamp
}))
```

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

## Recent Changes
- Initial project setup with all 5 packages
- Core library with types, crypto, encoding, merkle, dag, weight modules
- Node server with mempool, consensus, state management, and REST API
- Wallet library for key management and transaction creation
- Faucet for testnet coin distribution
- Explorer with DAG visualization, accounts view, and faucet integration
