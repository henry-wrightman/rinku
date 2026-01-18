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
}

export interface State {
  accounts: Account[];
  nodes: DAGNode[];
  tips: string[];
  tipUrls: string[];
  merkleRoot: string;
}
