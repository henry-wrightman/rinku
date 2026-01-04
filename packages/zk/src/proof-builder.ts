import type { ZkProofInput, MerkleWitness } from './types.js';
import { initEdDSA, generateKeyPair, signPoseidon, derivePublicKey, privateKeyFromSeed, type EdDSAKeyPair } from './eddsa.js';
import { initPoseidon, poseidonHash } from './poseidon.js';
import { CHAIN_ID_TESTNET } from './types.js';
import * as crypto from 'crypto';

export interface TransactionData {
  sender: string;
  recipient: string;
  amount: bigint;
  nonce: number;
}

export interface ZkProofContext {
  keyPair: EdDSAKeyPair;
  txHash: bigint;
  signature: {
    R8: [bigint, bigint];
    S: bigint;
  };
}

export async function initProofBuilder(): Promise<void> {
  await initPoseidon();
  await initEdDSA();
}

export async function createProofContext(txData: TransactionData, privateKeySeed?: string): Promise<ZkProofContext> {
  const keyPair = privateKeySeed 
    ? { privateKey: privateKeyFromSeed(privateKeySeed), publicKey: derivePublicKey(privateKeyFromSeed(privateKeySeed)) }
    : generateKeyPair();
  
  const txHash = await poseidonHash([
    BigInt('0x' + Buffer.from(txData.sender).slice(0, 20).toString('hex') || '0'),
    BigInt('0x' + Buffer.from(txData.recipient).slice(0, 20).toString('hex') || '0'),
    txData.amount,
    BigInt(txData.nonce)
  ]);
  
  const signature = signPoseidon(keyPair.privateKey, txHash);
  
  return { keyPair, txHash, signature };
}

export async function buildZkProofInput(
  context: ZkProofContext,
  merkleWitness: MerkleWitness,
  amount: bigint,
  chainId: bigint = BigInt(CHAIN_ID_TESTNET === 'rinku-testnet' ? 1 : 0)
): Promise<ZkProofInput> {
  const amountBlinding = BigInt('0x' + crypto.randomBytes(16).toString('hex'));
  
  const merklePathElements = merkleWitness.merklePathElements.map(el => {
    if (typeof el === 'string') {
      return el.startsWith('0x') ? BigInt(el) : BigInt('0x' + el);
    }
    return BigInt(el);
  });
  
  while (merklePathElements.length < 10) {
    merklePathElements.push(0n);
  }
  
  const merklePathIndices = merkleWitness.merklePathIndices.slice(0, 10);
  while (merklePathIndices.length < 10) {
    merklePathIndices.push(0);
  }
  
  return {
    txHash: context.txHash,
    senderPrivKey: BigInt('0x' + context.keyPair.privateKey.toString('hex')),
    senderPubKeyX: context.keyPair.publicKey[0],
    senderPubKeyY: context.keyPair.publicKey[1],
    txSigR8X: context.signature.R8[0],
    txSigR8Y: context.signature.R8[1],
    txSigS: context.signature.S,
    merklePathElements,
    merklePathIndices,
    amount,
    amountBlinding,
    checkpointHeight: BigInt(merkleWitness.checkpointHeight),
    chainId
  };
}

export async function createTestProofInput(): Promise<ZkProofInput> {
  await initProofBuilder();
  
  const txData: TransactionData = {
    sender: 'alice',
    recipient: 'bob',
    amount: 1000n,
    nonce: 1
  };
  
  const context = await createProofContext(txData, 'test-seed-for-determinism');
  
  const mockWitness: MerkleWitness = {
    txHash: context.txHash.toString(16),
    merklePathElements: Array(10).fill('0'),
    merklePathIndices: Array(10).fill(0),
    checkpointHeight: 100,
    checkpointRoot: '0'.repeat(64),
    checkpointId: 'test-checkpoint',
    chainId: 'rinku-testnet'
  };
  
  return buildZkProofInput(context, mockWitness, 1000n, 1n);
}
