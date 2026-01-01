export interface Account {
  fingerprint: string;
  balance: number;
  nonce: number;
  firstTxTimestamp: number;
}

export interface DAGNode {
  tx: {
    from: string;
    to: string;
    amount: number;
    nonce: number;
    tipUrls: string[];
    sig: string;
    ts: number;
    hash: string;
  };
  parentUrls: string[];
  children: string[];
  weight: number;
  confirmed: boolean;
  url: string;
}

export interface State {
  accounts: Account[];
  nodes: DAGNode[];
  tips: string[];
  tipUrls: string[];
  merkleRoot: string;
}
