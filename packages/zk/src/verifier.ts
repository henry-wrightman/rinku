import type { ZkProofPayload, ZkVerifyResult, ZkProof } from './types.js';
import { decodeZkUrl } from './encoding.js';
import { CHAIN_ID_TESTNET, ZK_URL_VERSION } from './types.js';

let snarkjs: typeof import('snarkjs') | null = null;
let verificationKey: object | null = null;

export async function initVerifier(vkeyPath: string): Promise<void> {
  snarkjs = await import('snarkjs');
  const fs = await import('fs');
  const vkeyJson = fs.readFileSync(vkeyPath, 'utf-8');
  verificationKey = JSON.parse(vkeyJson);
}

export async function initVerifierWithKey(vkey: object): Promise<void> {
  snarkjs = await import('snarkjs');
  verificationKey = vkey;
}

export function isVerifierInitialized(): boolean {
  return snarkjs !== null && verificationKey !== null;
}

export async function verifyZkUrl(url: string, expectedChainId?: string): Promise<ZkVerifyResult> {
  try {
    const payload = decodeZkUrl(url);
    return verifyZkPayload(payload, expectedChainId);
  } catch (error) {
    return {
      valid: false,
      reason: `Failed to decode URL: ${error instanceof Error ? error.message : 'Unknown error'}`
    };
  }
}

export async function verifyZkPayload(payload: ZkProofPayload, expectedChainId?: string): Promise<ZkVerifyResult> {
  if (!snarkjs || !verificationKey) {
    return { valid: false, reason: 'Verifier not initialized' };
  }

  if (payload.v !== ZK_URL_VERSION) {
    return { valid: false, reason: `Unsupported version: ${payload.v}` };
  }

  const chainId = expectedChainId || CHAIN_ID_TESTNET;
  if (payload.chainId !== chainId) {
    return { valid: false, reason: `Chain ID mismatch: expected ${chainId}, got ${payload.chainId}` };
  }

  try {
    const proof = deserializeProof(payload.proof);
    const publicSignals = [
      payload.publicInputs.cpRoot,
      payload.publicInputs.nullifier,
      payload.publicInputs.amountCommitment,
      payload.publicInputs.chainIdHash
    ];

    const isValid = await snarkjs.groth16.verify(verificationKey, publicSignals, proof);

    if (!isValid) {
      return { valid: false, reason: 'Groth16 proof verification failed' };
    }

    return {
      valid: true,
      cpHeight: payload.cpHeight,
      amountCommitment: payload.publicInputs.amountCommitment
    };
  } catch (error) {
    return {
      valid: false,
      reason: `Verification error: ${error instanceof Error ? error.message : 'Unknown error'}`
    };
  }
}

function deserializeProof(proofData: string): ZkProof {
  if (proofData.startsWith('{')) {
    const parsed = JSON.parse(proofData);
    return {
      pi_a: parsed.pi_a,
      pi_b: parsed.pi_b,
      pi_c: parsed.pi_c,
      protocol: parsed.protocol || 'groth16',
      curve: parsed.curve || 'bn128'
    };
  }
  
  const buffer = Buffer.from(proofData, 'base64');
  const view = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);

  let offset = 0;
  const pi_a: [string, string, string] = [
    readBigInt(view, offset, 32).toString(),
    readBigInt(view, offset + 32, 32).toString(),
    '1'
  ];
  offset += 64;

  const pi_b: [[string, string], [string, string], [string, string]] = [
    [
      readBigInt(view, offset, 32).toString(),
      readBigInt(view, offset + 32, 32).toString()
    ],
    [
      readBigInt(view, offset + 64, 32).toString(),
      readBigInt(view, offset + 96, 32).toString()
    ],
    ['1', '0']
  ];
  offset += 128;

  const pi_c: [string, string, string] = [
    readBigInt(view, offset, 32).toString(),
    readBigInt(view, offset + 32, 32).toString(),
    '1'
  ];

  return {
    pi_a,
    pi_b,
    pi_c,
    protocol: 'groth16',
    curve: 'bn128'
  };
}

function readBigInt(view: DataView, offset: number, bytes: number): bigint {
  let result = 0n;
  for (let i = 0; i < bytes; i++) {
    result = (result << 8n) | BigInt(view.getUint8(offset + i));
  }
  return result;
}

export class NullifierRegistry {
  private nullifiers = new Set<string>();

  has(nullifier: string): boolean {
    return this.nullifiers.has(nullifier);
  }

  add(nullifier: string): void {
    this.nullifiers.add(nullifier);
  }

  remove(nullifier: string): void {
    this.nullifiers.delete(nullifier);
  }

  clear(): void {
    this.nullifiers.clear();
  }

  size(): number {
    return this.nullifiers.size;
  }
}

export const globalNullifierRegistry = new NullifierRegistry();

export async function verifyZkUrlWithNullifier(
  url: string,
  registry: NullifierRegistry = globalNullifierRegistry,
  expectedChainId?: string
): Promise<ZkVerifyResult> {
  const payload = decodeZkUrl(url);
  
  if (registry.has(payload.publicInputs.nullifier)) {
    return { valid: false, reason: 'Nullifier already used (double-claim attempt)' };
  }

  const result = await verifyZkPayload(payload, expectedChainId);
  
  if (result.valid) {
    registry.add(payload.publicInputs.nullifier);
  }
  
  return result;
}
