import type {
  Checkpoint,
  CheckpointProof,
  CheckpointConfig,
  CheckpointVerification
} from './types.js';
import { hash } from './crypto.js';

export const DEFAULT_CHECKPOINT_CONFIG: CheckpointConfig = {
  checkpointIntervalMs: 60000,
  minSignaturesRequired: 1,
  minValidatorWeightPercent: 51
};

export async function createCheckpointId(
  height: number,
  merkleRoot: string,
  timestamp: number
): Promise<string> {
  const data = `checkpoint:${height}:${merkleRoot}:${timestamp}`;
  const fullHash = await hash(data);
  return fullHash.slice(0, 16);
}

export async function createCheckpoint(
  height: number,
  merkleRoot: string,
  tipUrls: string[],
  totalTransactions: number,
  totalWeight: number
): Promise<Checkpoint> {
  const timestamp = Date.now();
  return {
    checkpointId: await createCheckpointId(height, merkleRoot, timestamp),
    height,
    merkleRoot,
    tipUrls,
    totalTransactions,
    totalWeight,
    timestamp,
    signatures: []
  };
}

export function getCheckpointSigningData(checkpoint: Checkpoint): string {
  return JSON.stringify({
    checkpointId: checkpoint.checkpointId,
    height: checkpoint.height,
    merkleRoot: checkpoint.merkleRoot,
    tipUrls: checkpoint.tipUrls.sort(),
    totalTransactions: checkpoint.totalTransactions,
    totalWeight: checkpoint.totalWeight,
    timestamp: checkpoint.timestamp
  });
}

export function createCheckpointProof(
  checkpoint: Checkpoint,
  totalValidatorWeight: number
): CheckpointProof {
  return {
    checkpointId: checkpoint.checkpointId,
    checkpointHeight: checkpoint.height,
    merkleRoot: checkpoint.merkleRoot,
    signatureCount: checkpoint.signatures.length,
    totalValidatorWeight,
    signatures: checkpoint.signatures
  };
}

export function verifyCheckpointProof(
  proof: CheckpointProof,
  config: CheckpointConfig = DEFAULT_CHECKPOINT_CONFIG
): CheckpointVerification {
  const errors: string[] = [];

  if (proof.signatureCount < config.minSignaturesRequired) {
    errors.push(
      `Insufficient signatures: ${proof.signatureCount} < ${config.minSignaturesRequired} required`
    );
  }

  const weightPercent = proof.totalValidatorWeight;
  if (weightPercent < config.minValidatorWeightPercent) {
    errors.push(
      `Insufficient validator weight: ${weightPercent.toFixed(1)}% < ${config.minValidatorWeightPercent}% required`
    );
  }

  return {
    valid: errors.length === 0,
    checkpointId: proof.checkpointId,
    signatureCount: proof.signatureCount,
    validatorWeightPercent: weightPercent,
    errors
  };
}

export function encodeCheckpointProof(proof: CheckpointProof): string {
  const compactProof = {
    c: proof.checkpointId,
    h: proof.checkpointHeight,
    m: proof.merkleRoot,
    n: proof.signatureCount,
    w: proof.totalValidatorWeight,
    s: proof.signatures.map(sig => ({
      v: sig.validator,
      g: sig.signature,
      p: sig.publicKey,
      t: sig.timestamp
    }))
  };
  return Buffer.from(JSON.stringify(compactProof)).toString('base64url');
}

export function decodeCheckpointProof(encoded: string): CheckpointProof | null {
  try {
    const json = Buffer.from(encoded, 'base64url').toString('utf-8');
    const compact = JSON.parse(json);
    return {
      checkpointId: compact.c,
      checkpointHeight: compact.h,
      merkleRoot: compact.m,
      signatureCount: compact.n,
      totalValidatorWeight: compact.w,
      signatures: compact.s.map((sig: any) => ({
        validator: sig.v,
        signature: sig.g,
        publicKey: sig.p,
        timestamp: sig.t
      }))
    };
  } catch {
    return null;
  }
}

export function embedProofInUrl(txUrl: string, proof: CheckpointProof): string {
  const encodedProof = encodeCheckpointProof(proof);
  const separator = txUrl.includes('?') ? '&' : '?';
  return `${txUrl}${separator}proof=${encodedProof}`;
}

export function extractProofFromUrl(url: string): {
  txUrl: string;
  proof: CheckpointProof | null;
} {
  const proofMatch = url.match(/[?&]proof=([^&]+)/);
  if (!proofMatch) {
    return { txUrl: url, proof: null };
  }

  const txUrl = url.replace(/[?&]proof=[^&]+/, '');
  const proof = decodeCheckpointProof(proofMatch[1]);
  return { txUrl, proof };
}
