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

function addressToBigInt(address: string): bigint {
  if (!address) return 0n;
  
  const clean = address.replace(/^0x/i, '');
  
  if (/^[0-9a-fA-F]{40}$/.test(clean)) {
    return BigInt('0x' + clean.padStart(64, '0'));
  }
  
  if (/^[0-9a-fA-F]+$/.test(clean)) {
    const normalized = clean.length <= 64 ? clean.padStart(64, '0') : clean.slice(-64);
    return BigInt('0x' + normalized);
  }
  
  const bytes = Buffer.from(address, 'utf8');
  const hex = bytes.toString('hex');
  const normalized = hex.length <= 64 ? hex.padStart(64, '0') : hex.slice(-64);
  return BigInt('0x' + normalized);
}

export async function createProofContext(txData: TransactionData, privateKeySeed?: string): Promise<ZkProofContext> {
  const keyPair = privateKeySeed 
    ? { privateKey: privateKeyFromSeed(privateKeySeed), publicKey: derivePublicKey(privateKeyFromSeed(privateKeySeed)) }
    : generateKeyPair();
  
  const senderInt = addressToBigInt(txData.sender);
  const recipientInt = addressToBigInt(txData.recipient);
  
  const txHash = await poseidonHash([
    senderInt,
    recipientInt,
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
