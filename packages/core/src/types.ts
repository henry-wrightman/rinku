export interface AccountState {
  fingerprint: string;
  balance: number;
  nonce: number;
  firstTxTimestamp: number;
}

export interface Transaction {
  from: string;
  to: string;
  amount: number;
  nonce: number;
  tipUrls: string[];
  sig: string;
  ts: number;
}

export interface SignedTransaction extends Transaction {
  hash: string;
}

export interface DAGNode {
  tx: SignedTransaction;
  parentUrls: string[];
  children: string[];
  weight: number;
  confirmed: boolean;
}

export interface MerkleNode {
  hash: string;
  left?: MerkleNode;
  right?: MerkleNode;
  data?: string;
}

export interface KeyPair {
  publicKey: Uint8Array;
  privateKey: Uint8Array;
  fingerprint: string;
}

export interface NodeState {
  accounts: Map<string, AccountState>;
  dag: Map<string, DAGNode>;
  tipUrls: Set<string>;
  merkleRoot: string;
}

export interface TransactionURL {
  path: string;
  payload: string;
}

export type Weight = {
  accountAge: number;
  balance: number;
  total: number;
};

// ============================================
// Smart Contract Types
// ============================================

/** Contract deployment payload */
export interface ContractDeploy {
  type: 'deploy';
  contractId: string;        // Unique contract identifier (derived from creator + nonce)
  creator: string;           // Creator's fingerprint
  wasmBase64: string;        // WASM bytecode as base64
  initState: Record<string, unknown>;  // Initial contract state
  tipUrls: string[];         // Parent transaction URLs
  sig: string;               // Creator's signature
  ts: number;                // Timestamp
}

/** Contract call payload - embedded in regular transaction */
export interface ContractCall {
  action: 'call';
  contractId: string;        // Target contract
  entrypoint: string;        // Function to call (e.g., "transfer", "mint")
  input: Record<string, unknown>;  // Call arguments
  preStateHash: string;      // Hash of state before execution
  postStateHash: string;     // Hash of state after execution
}

/** State diff produced by contract execution */
export interface StateDiff {
  contractId: string;
  height: number;            // Execution sequence number
  changes: StateChange[];    // Key-value changes
  preHash: string;           // State hash before
  postHash: string;          // State hash after
}

export interface StateChange {
  key: string;
  oldValue: unknown;
  newValue: unknown;
}

/** Extended transaction that can include contract calls */
export interface ContractTransaction extends SignedTransaction {
  contract?: ContractCall;   // Optional contract call data
}

/** Contract metadata stored by nodes */
export interface ContractState {
  contractId: string;
  creator: string;
  wasmBase64: string;
  deployUrl: string;         // URL where contract was deployed
  state: Record<string, unknown>;  // Current contract state
  stateHash: string;         // Merkle hash of current state
  height: number;            // Number of state transitions
  createdAt: number;
}

/** Contract URL types */
export interface ContractURL {
  path: string;
  payload: string;
}

/** WASM execution result */
export interface ExecutionResult {
  success: boolean;
  stateDiff: StateDiff | null;
  gasUsed: number;
  error?: string;
  logs: string[];
}
