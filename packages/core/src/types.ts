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
  fee: number;
  nonce: number;
  tipUrls: string[];
  sig: string;
  ts: number;
}

// ============================================
// Gas & Fee Types
// ============================================

/** Current gas price from network oracle */
export interface GasPrice {
  current: number;
  min: number;
  max: number;
  avgLast100: number;
  lastUpdated: number;
}

/** Gas configuration */
export interface GasConfig {
  minFee: number;
  maxFee: number;
  baseFee: number;
  feeMultiplier: number;
  burnPercent: number;
  validatorPercent: number;
}

/** Fee statistics */
export interface FeeStats {
  totalBurned: number;
  totalToValidators: number;
  avgFee: number;
  txCount: number;
}

export interface SignedTransaction extends Transaction {
  hash: string;
}

/** Finality metadata stamped when transaction is included in a finalized checkpoint */
export interface FinalityMetadata {
  checkpointId: string;
  checkpointHeight: number;
  finalizedAt: number;
}

export interface DAGNode {
  tx: SignedTransaction;
  parentUrls: string[];
  children: string[];
  weight: number;
  confirmed: boolean;
  url?: string;
  finality?: FinalityMetadata;
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

// ============================================
// Contract Receipt Types (Layer 1 Extension)
// ============================================

/** Structured contract event emitted during execution */
export interface ContractEvent {
  contractId: string;
  eventName: string;
  data: Record<string, unknown>;
  index: number;  // Position in event sequence
}

/** Canonical contract execution receipt - the verifiable output of a contract call */
export interface ContractReceipt {
  // Unique identification
  callId: string;              // Hash of (txHash + checkpointHeight) - unique call identifier
  txHash: string;              // Transaction that triggered this execution
  contractId: string;          // Contract that was called
  
  // Execution details
  entrypoint: string;          // Function that was called
  caller: string;              // Address that initiated the call
  
  // State transition
  preStateRoot: string;        // Contract state root before execution
  postStateRoot: string;       // Contract state root after execution
  effectsHash: string;         // Hash of touched keys + their deltas
  
  // Execution outcome
  status: 'success' | 'revert' | 'out_of_gas';
  gasUsed: number;
  gasLimit: number;
  
  // Events (compact form - full events optional)
  eventsHash: string;          // Hash of all emitted events
  eventCount: number;          // Number of events emitted
  events?: ContractEvent[];    // Full events (optional, can be omitted for compact receipts)
  
  // Error info (only on failure)
  revertReason?: string;
  
  // Timing
  executedAt: number;          // Timestamp of execution
}

/** Touched storage key with before/after values for witness proofs */
export interface TouchedKey {
  contractId: string;
  key: string;
  preValue: unknown;
  postValue: unknown;
}

/** State witness for Profile B proofs - proves specific state changes */
export interface StateWitness {
  touchedKeys: TouchedKey[];
  merkleProofs: {
    key: string;
    proof: string[];   // Merkle path from key to stateRoot
    index: number;
  }[];
}

/** Extended receipt with witness for Profile B verification */
export interface ContractReceiptWithWitness extends ContractReceipt {
  witness: StateWitness;
}

// ============================================
// Rewards & Staking Types
// ============================================

/** Reward earned for validating a tip (referencing an orphaned transaction) */
export interface TipReward {
  type: 'tip';
  recipient: string;
  amount: number;
  txUrl: string;
  tipUrl: string;
  timestamp: number;
}

/** Reward earned for staking tokens and validating transactions */
export interface StakeReward {
  type: 'stake';
  recipient: string;
  amount: number;
  validatedTxUrl: string;
  timestamp: number;
}

/** Reward earned when your transaction is referenced by others */
export interface WitnessReward {
  type: 'witness';
  recipient: string;
  amount: number;
  witnessedTxUrl: string;
  referencedByUrl: string;
  timestamp: number;
}

export type Reward = TipReward | StakeReward | WitnessReward;

/** Staking position for a validator */
export interface StakePosition {
  staker: string;
  amount: number;
  stakedAt: number;
  lastRewardAt: number;
}

/** Configuration for reward rates */
export interface RewardConfig {
  tipRewardRate: number;
  stakeRewardRate: number;
  witnessRewardRate: number;
  minStakeAmount: number;
  unstakeCooldownMs: number;
}

/** Account rewards summary */
export interface RewardsSummary {
  address: string;
  tipRewards: number;
  stakeRewards: number;
  witnessRewards: number;
  totalRewards: number;
  pendingRewards: number;
  rewardHistory: Reward[];
}

/** Staking status for an account */
export interface StakingStatus {
  address: string;
  stakedAmount: number;
  isValidator: boolean;
  stakedAt: number | null;
  earnedRewards: number;
  canUnstakeAt: number | null;
}

// ============================================
// Checkpoint & Finality Types
// ============================================

/** A validator's signature on a checkpoint */
export interface ValidatorSignature {
  validator: string;
  signature: string;
  publicKey: number[];
  weight: number;
  timestamp: number;
}

/** A validator entry for checkpoint authentication */
export interface ValidatorEntry {
  address: string;
  publicKey: number[];
  weight: number;
}

/** A checkpoint representing network consensus at a point in time */
export interface Checkpoint {
  checkpointId: string;
  height: number;
  merkleRoot: string;
  txMerkleRoot?: string;
  txHashes?: string[];
  stateRoot?: string;           // Root of global contract state trie
  receiptRoot?: string;         // Root of receipts trie for this checkpoint
  tipCount: number;
  totalTransactions: number;
  totalWeight: number;
  validatorSetHash: string;
  previousCheckpointId: string | null;
  validators: ValidatorEntry[];
  timestamp: number;
  signatures: ValidatorSignature[];
}

/** Compact proof that a transaction is part of canonical history */
export interface CheckpointProof {
  checkpointId: string;
  checkpointHeight: number;
  merkleRoot: string;
  txMerkleRoot?: string;
  signatureCount: number;
  totalValidatorWeight: number;
  totalNetworkWeight: number;
  validatorSetHash: string;
  previousCheckpointId: string | null;
  validators: ValidatorEntry[];
  signatures: ValidatorSignature[];
}

/** Genesis configuration for bootstrapping trust */
export interface GenesisConfig {
  chainId: string;
  genesisTime: number;
  initialValidators: ValidatorEntry[];
  genesisCheckpointId: string;
}

/** Extended transaction URL with embedded finality proof */
export interface FinalizedTransactionURL {
  path: string;
  payload: string;
  proof?: CheckpointProof;
}

/** Checkpoint anchor info for proof bundles */
export interface CheckpointAnchor {
  checkpointId: string;
  merkleRoot: string;
  txMerkleRoot?: string;
  stateRoot?: string;           // For contract state proofs
  receiptRoot?: string;         // For receipt inclusion proofs
  height: number;
  signatureCount: number;
}

/** Merkle inclusion proof for a transaction */
export interface TransactionMerkleProof {
  proof: string[];
  index: number;
  txMerkleRoot: string;
}

/** Reference to a truncated (finalized) parent with self-contained verification data */
export interface TruncatedParentRef {
  hash: string;
  tx: Transaction;
  checkpointAnchor: CheckpointAnchor;
  merkleProof?: TransactionMerkleProof;
}

/** Self-crawlable transaction bundle - contains full ancestry back to checkpoint(s) */
export interface SelfCrawlableBundle {
  tx: Transaction;
  hash: string;
  parents: SelfCrawlableBundle[];
  truncatedParents?: TruncatedParentRef[];
  checkpointAnchor?: CheckpointAnchor;
}

/** Configuration for checkpoint system */
export interface CheckpointConfig {
  checkpointIntervalMs: number;
  minSignaturesRequired: number;
  minValidatorWeightPercent: number;
}

/** Checkpoint verification result */
export interface CheckpointVerification {
  valid: boolean;
  checkpointId: string;
  signatureCount: number;
  validatorWeightPercent: number;
  errors: string[];
}
