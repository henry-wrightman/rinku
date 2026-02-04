export interface Account {
  fingerprint: string;
  balance: number;
  nonce: number;
  firstTxTimestamp: number;
}

export type TransactionKind = 'transfer' | 'stake' | 'unstake' | 'claim_rewards' | 'contract' | 'consolidation' | 'reward';

export type FastPathStatus = 'pending' | 'confirmed' | 'timeout' | 'not_eligible';

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
