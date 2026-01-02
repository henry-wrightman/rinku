export interface Account {
  fingerprint: string;
  balance: number;
  nonce: number;
  firstTxTimestamp: number;
}

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
}

export interface State {
  accounts: Account[];
  nodes: DAGNode[];
  tips: string[];
  tipUrls: string[];
  merkleRoot: string;
}
