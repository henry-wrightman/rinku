import type {
  Checkpoint,
  CheckpointProof,
  CheckpointConfig,
  CheckpointVerification
} from './types.js';
import { hash, verify, computeFingerprint } from './crypto.js';

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

export async function computeValidatorSetHash(
  validators: { address: string; weight: number }[]
): Promise<string> {
  const sorted = [...validators].sort((a, b) => a.address.localeCompare(b.address));
  const data = sorted.map(v => `${v.address}:${v.weight}`).join(',');
  return await hash(data);
}

export async function createCheckpoint(
  height: number,
  merkleRoot: string,
  tipUrls: string[],
  totalTransactions: number,
  totalWeight: number,
  validators: { address: string; weight: number }[]
): Promise<Checkpoint> {
  const timestamp = Date.now();
  return {
    checkpointId: await createCheckpointId(height, merkleRoot, timestamp),
    height,
    merkleRoot,
    tipUrls,
    totalTransactions,
    totalWeight,
    validatorSetHash: await computeValidatorSetHash(validators),
    timestamp,
    signatures: []
  };
}

export function getCheckpointSigningData(checkpoint: Checkpoint | CheckpointProof): string {
  const height = 'height' in checkpoint ? (checkpoint as Checkpoint).height : (checkpoint as CheckpointProof).checkpointHeight;
  const totalWeight = 'totalWeight' in checkpoint ? (checkpoint as Checkpoint).totalWeight : (checkpoint as CheckpointProof).totalNetworkWeight;
  return JSON.stringify({
    checkpointId: checkpoint.checkpointId,
    height,
    merkleRoot: checkpoint.merkleRoot,
    totalWeight,
    validatorSetHash: checkpoint.validatorSetHash
  });
}

export function createCheckpointProof(
  checkpoint: Checkpoint,
  validatorWeightPercent: number
): CheckpointProof {
  return {
    checkpointId: checkpoint.checkpointId,
    checkpointHeight: checkpoint.height,
    merkleRoot: checkpoint.merkleRoot,
    signatureCount: checkpoint.signatures.length,
    totalValidatorWeight: validatorWeightPercent,
    totalNetworkWeight: checkpoint.totalWeight,
    validatorSetHash: checkpoint.validatorSetHash,
    signatures: checkpoint.signatures
  };
}

export async function verifyCheckpointProof(
  proof: CheckpointProof,
  config: CheckpointConfig = DEFAULT_CHECKPOINT_CONFIG
): Promise<CheckpointVerification> {
  const errors: string[] = [];
  let validSignatures = 0;
  let verifiedWeight = 0;

  const validatorWeights: { address: string; weight: number }[] = proof.signatures.map(s => ({
    address: s.validator,
    weight: s.weight
  }));
  const computedSetHash = await computeValidatorSetHash(validatorWeights);
  
  const signingData = getCheckpointSigningData(proof);

  for (const sig of proof.signatures) {
    try {
      const publicKeyBytes = new Uint8Array(sig.publicKey);
      
      const expectedFingerprint = await computeFingerprint(publicKeyBytes);
      if (expectedFingerprint !== sig.validator) {
        errors.push(`Signature from ${sig.validator.slice(0, 8)}... has mismatched public key`);
        continue;
      }

      const signerSigningData = signingData + `:${sig.weight}`;
      const isValid = await verify(signerSigningData, sig.signature, publicKeyBytes);
      if (isValid) {
        validSignatures++;
        verifiedWeight += sig.weight || 0;
      } else {
        errors.push(`Invalid signature from ${sig.validator.slice(0, 8)}...`);
      }
    } catch (err) {
      errors.push(`Failed to verify signature from ${sig.validator.slice(0, 8)}...`);
    }
  }

  if (validSignatures < config.minSignaturesRequired) {
    errors.push(
      `Insufficient valid signatures: ${validSignatures} < ${config.minSignaturesRequired} required`
    );
  }

  const totalNetworkWeight = proof.totalNetworkWeight || 0;
  const computedWeightPercent = totalNetworkWeight > 0 
    ? (verifiedWeight / totalNetworkWeight) * 100 
    : 0;
    
  if (computedWeightPercent < config.minValidatorWeightPercent) {
    errors.push(
      `Insufficient verified weight: ${computedWeightPercent.toFixed(1)}% < ${config.minValidatorWeightPercent}% required`
    );
  }

  return {
    valid: validSignatures >= config.minSignaturesRequired && 
           computedWeightPercent >= config.minValidatorWeightPercent,
    checkpointId: proof.checkpointId,
    signatureCount: validSignatures,
    validatorWeightPercent: computedWeightPercent,
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
    t: proof.totalNetworkWeight,
    v: proof.validatorSetHash,
    s: proof.signatures.map(sig => ({
      v: sig.validator,
      g: sig.signature,
      p: sig.publicKey,
      w: sig.weight || 0,
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
      totalNetworkWeight: compact.t || 0,
      validatorSetHash: compact.v || '',
      signatures: compact.s.map((sig: any) => ({
        validator: sig.v,
        signature: sig.g,
        publicKey: sig.p,
        weight: sig.w || 0,
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
