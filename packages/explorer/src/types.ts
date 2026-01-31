export interface Account {
  fingerprint: string;
  balance: number;
  nonce: number;
  firstTxTimestamp: number;
}

export type TransactionKind = 'transfer' | 'stake' | 'unstake' | 'claim_rewards' | 'contract' | 'consolidation' | 'reward';

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
  kind?: TransactionKind;
  memo?: string;
  references?: string[];
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
