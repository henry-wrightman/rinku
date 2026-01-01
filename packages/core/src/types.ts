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
  tips: string[];
  sig: string;
  ts: number;
}

export interface SignedTransaction extends Transaction {
  hash: string;
}

export interface DAGNode {
  tx: SignedTransaction;
  parents: string[];
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
  tips: Set<string>;
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
