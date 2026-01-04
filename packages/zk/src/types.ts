export interface ZkProofInput {
  txHash: bigint;
  senderPrivKey: bigint;
  senderPubKeyX: bigint;
  senderPubKeyY: bigint;
  txSigR8X: bigint;
  txSigR8Y: bigint;
  txSigS: bigint;
  
  merklePathElements: bigint[];
  merklePathIndices: number[];
  
  amount: bigint;
  amountBlinding: bigint;
  
  checkpointHeight: bigint;
  chainId: bigint;
}

export interface ZkProofOutput {
  checkpointRoot: bigint;
  nullifier: bigint;
  amountCommitment: bigint;
  chainIdHash: bigint;
}

export interface ZkProof {
  pi_a: [string, string, string];
  pi_b: [[string, string], [string, string], [string, string]];
  pi_c: [string, string, string];
  protocol: string;
  curve: string;
}

export interface ZkProofPayload {
  v: number;
  chainId: string;
  cpHeight: number;
  proof: string;
  publicInputs: {
    cpRoot: string;
    nullifier: string;
    amountCommitment: string;
    chainIdHash: string;
  };
  encryptedMemo?: string;
  auxData: {
    validatorRoot: string;
    totalWeight: number;
  };
}

export interface ZkVerifyResult {
  valid: boolean;
  reason?: string;
  cpHeight?: number;
  amountCommitment?: string;
}

export interface MerkleWitness {
  txHash: string;
  merklePathElements: string[];
  merklePathIndices: number[];
  checkpointHeight: number;
  checkpointRoot: string;
  checkpointId?: string;
  chainId?: string;
}

export const ZK_URL_VERSION = 1;
export const MERKLE_DEPTH = 10;
export const CHAIN_ID_MAINNET = 'rinku-mainnet';
export const CHAIN_ID_TESTNET = 'rinku-testnet';
