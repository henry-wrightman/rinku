import type { ZkProofInput, ZkProof, ZkProofPayload, MerkleWitness } from './types.js';
import { initPoseidon, computeNullifier, computeAmountCommitment, computeChainIdHash, computeMerkleRoot } from './poseidon.js';
import { encodeZkUrl } from './encoding.js';
import { ZK_URL_VERSION, CHAIN_ID_TESTNET } from './types.js';

let snarkjs: typeof import('snarkjs') | null = null;
let wasmPath: string | null = null;
let zkeyPath: string | null = null;

export async function initProver(circuitWasmPath: string, circuitZkeyPath: string): Promise<void> {
  snarkjs = await import('snarkjs');
  wasmPath = circuitWasmPath;
  zkeyPath = circuitZkeyPath;
  await initPoseidon();
}

export function isProverInitialized(): boolean {
  return snarkjs !== null && wasmPath !== null && zkeyPath !== null;
}

export async function generateProof(input: ZkProofInput): Promise<{ proof: ZkProof; publicSignals: string[] }> {
  if (!snarkjs || !wasmPath || !zkeyPath) {
    throw new Error('Prover not initialized. Call initProver() first.');
  }

  const circuitInput = {
    txHash: input.txHash.toString(),
    senderPrivKey: input.senderPrivKey.toString(),
    senderPubKeyX: input.senderPubKeyX.toString(),
    senderPubKeyY: input.senderPubKeyY.toString(),
    txSigR8X: input.txSigR8X.toString(),
    txSigR8Y: input.txSigR8Y.toString(),
    txSigS: input.txSigS.toString(),
    merklePathElements: input.merklePathElements.map(e => e.toString()),
    merklePathIndices: input.merklePathIndices,
    amount: input.amount.toString(),
    amountBlinding: input.amountBlinding.toString(),
    checkpointHeight: input.checkpointHeight.toString(),
    chainId: input.chainId.toString()
  };

  const { proof, publicSignals } = await snarkjs.groth16.fullProve(
    circuitInput,
    wasmPath,
    zkeyPath
  );

  return { proof: proof as ZkProof, publicSignals };
}

export async function generateZkUrl(
  input: ZkProofInput,
  validatorRoot: string,
  totalWeight: number,
  encryptedMemo?: string
): Promise<string> {
  const { proof, publicSignals } = await generateProof(input);
  
  const payload: ZkProofPayload = {
    v: ZK_URL_VERSION,
    chainId: CHAIN_ID_TESTNET,
    cpHeight: Number(input.checkpointHeight),
    proof: serializeProof(proof),
    publicInputs: {
      cpRoot: publicSignals[0],
      nullifier: publicSignals[1],
      amountCommitment: publicSignals[2],
      chainIdHash: publicSignals[3]
    },
    encryptedMemo,
    auxData: {
      validatorRoot,
      totalWeight
    }
  };
  
  return encodeZkUrl(payload);
}

export function computeExpectedOutputs(input: ZkProofInput): {
  checkpointRoot: bigint;
  nullifier: bigint;
  amountCommitment: bigint;
  chainIdHash: bigint;
} {
  const checkpointRoot = computeMerkleRoot(
    input.txHash,
    input.merklePathElements,
    input.merklePathIndices
  );
  
  const nullifier = computeNullifier(
    input.senderPrivKey,
    input.checkpointHeight,
    input.txHash
  );
  
  const amountCommitment = computeAmountCommitment(
    input.amount,
    input.amountBlinding
  );
  
  const chainIdHash = computeChainIdHash(input.chainId);
  
  return { checkpointRoot, nullifier, amountCommitment, chainIdHash };
}

function serializeProof(proof: ZkProof): string {
  const buffer = new ArrayBuffer(192);
  const view = new DataView(buffer);
  
  const pi_a = proof.pi_a.slice(0, 2).map(s => BigInt(s));
  const pi_b = proof.pi_b.slice(0, 2).map(row => row.map(s => BigInt(s)));
  const pi_c = proof.pi_c.slice(0, 2).map(s => BigInt(s));
  
  let offset = 0;
  for (const val of pi_a) {
    writeBigInt(view, offset, val, 32);
    offset += 32;
  }
  for (const row of pi_b) {
    for (const val of row) {
      writeBigInt(view, offset, val, 32);
      offset += 32;
    }
  }
  for (const val of pi_c) {
    writeBigInt(view, offset, val, 32);
    offset += 32;
  }
  
  return Buffer.from(buffer).toString('base64');
}

function writeBigInt(view: DataView, offset: number, value: bigint, bytes: number): void {
  for (let i = bytes - 1; i >= 0; i--) {
    view.setUint8(offset + i, Number(value & 0xffn));
    value >>= 8n;
  }
}

export async function fetchMerkleWitness(nodeUrl: string, txHash: string): Promise<MerkleWitness> {
  const response = await fetch(`${nodeUrl}/api/zk/witness/${txHash}`);
  if (!response.ok) {
    throw new Error(`Failed to fetch Merkle witness: ${response.statusText}`);
  }
  return response.json() as Promise<MerkleWitness>;
}
