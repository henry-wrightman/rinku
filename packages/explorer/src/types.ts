export interface Account {
  fingerprint: string;
  balance: number;
  nonce: number;
  firstTxTimestamp: number;
}

export type TransactionKind = 'transfer' | 'stake' | 'unstake' | 'claim_rewards' | 'contract' | 'consolidation' | 'reward';

export type FastPathStatus = 'pending' | 'confirmed' | 'executed' | 'finalized' | 'timeout' | 'not_eligible';

export interface DAGNode {
  hash: string;
  from: string;
  to: string;
  amount: number;
  fee: number;
  ts: number;
  parentCount: number;
  url: string;
  weight: number;
  confirmed: boolean;
  finalized: boolean;
  kind?: TransactionKind;
  memo?: string;
  references?: string[];
  fast_path_status?: FastPathStatus;
  fast_path_confirmed_at_ms?: number;
  fast_path_finality_ms?: number;
  trust_score?: number;
  attestation_count?: number;
}

export interface AggregatedWeight {
  boost_stake_micro: number;
  suppress_stake_micro: number;
  neutral_stake_micro: number;
  net_weight: number;
  attestation_count: number;
  total_network_stake_micro: number;
}

export interface WeightProofResponse {
  tx_hash: string;
  aggregated_weight: AggregatedWeight;
  trust_score: number;
  boost_ratio: number;
  suppress_ratio: number;
  checkpoint_height?: number;
  weight_trie_root: string;
  merkle_proof: string[];
  merkle_index: number;
}

export interface ThreadTransaction {
  hash: string;
  from: string;
  to: string;
  amount: number;
  fee: number;
  nonce: number;
  ts: number;
  tipUrls: string[];
  finalized: boolean;
  weight: number;
  url: string;
  memo?: string;
  references?: string[];
  fast_path_status?: FastPathStatus;
  fast_path_confirmed_at_ms?: number;
  fast_path_finality_ms?: number;
}

export interface ThreadResponse {
  parentHash: string;
  replies: ThreadTransaction[];
  totalReplies: number;
}

export interface State {
  accounts: Account[];
  nodes: DAGNode[];
  tips: string[];
  tipUrls: string[];
  merkleRoot: string;
}
