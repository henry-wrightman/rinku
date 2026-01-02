import type {
  Checkpoint,
  CheckpointProof,
  CheckpointConfig,
  CheckpointVerification,
  ValidatorEntry,
  GenesisConfig
} from './types.js';
import { hash, verify, computeFingerprint } from './crypto.js';

export const DEFAULT_CHECKPOINT_CONFIG: CheckpointConfig = {
  checkpointIntervalMs: 60000,
  minSignaturesRequired: 1,
  minValidatorWeightPercent: 51
};

export const GENESIS_CHECKPOINT_ID = 'genesis_00000000';

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
  validators: ValidatorEntry[]
): Promise<string> {
  const sorted = [...validators].sort((a, b) => a.address.localeCompare(b.address));
  const data = sorted.map(v => `${v.address}:${v.weight}`).join(',');
  return await hash(data);
}

export async function createGenesisCheckpoint(
  chainId: string,
  initialValidators: ValidatorEntry[]
): Promise<Checkpoint> {
  const timestamp = Date.now();
  return {
    checkpointId: GENESIS_CHECKPOINT_ID,
    height: 0,
    merkleRoot: 'genesis',
    tipCount: 0,
    totalTransactions: 0,
    totalWeight: initialValidators.reduce((sum, v) => sum + v.weight, 0),
    validatorSetHash: await computeValidatorSetHash(initialValidators),
    previousCheckpointId: null,
    validators: initialValidators,
    timestamp,
    signatures: []
  };
}

export async function createCheckpoint(
  height: number,
  merkleRoot: string,
  tipCount: number,
  totalTransactions: number,
  totalWeight: number,
  validators: ValidatorEntry[],
  previousCheckpointId: string | null
): Promise<Checkpoint> {
  const timestamp = Date.now();
  return {
    checkpointId: await createCheckpointId(height, merkleRoot, timestamp),
    height,
    merkleRoot,
    tipCount,
    totalTransactions,
    totalWeight,
    validatorSetHash: await computeValidatorSetHash(validators),
    previousCheckpointId,
    validators,
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
    validatorSetHash: checkpoint.validatorSetHash,
    previousCheckpointId: checkpoint.previousCheckpointId
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
    previousCheckpointId: checkpoint.previousCheckpointId,
    validators: checkpoint.validators,
    signatures: checkpoint.signatures
  };
}

export async function verifyCheckpointProof(
  proof: CheckpointProof,
  trustedValidators: ValidatorEntry[],
  config: CheckpointConfig = DEFAULT_CHECKPOINT_CONFIG
): Promise<CheckpointVerification> {
  const errors: string[] = [];
  let validSignatures = 0;
  let verifiedWeight = 0;

  const trustedSetHash = await computeValidatorSetHash(trustedValidators);
  if (trustedSetHash !== proof.validatorSetHash) {
    errors.push(`Validator set mismatch: proof has ${proof.validatorSetHash.slice(0, 8)}..., trusted is ${trustedSetHash.slice(0, 8)}...`);
  }

  const trustedValidatorMap = new Map(
    trustedValidators.map(v => [v.address, v])
  );

  const signingData = getCheckpointSigningData(proof);

  for (const sig of proof.signatures) {
    try {
      const trustedValidator = trustedValidatorMap.get(sig.validator);
      if (!trustedValidator) {
        errors.push(`Unknown validator: ${sig.validator.slice(0, 8)}...`);
        continue;
      }

      if (Math.abs(trustedValidator.weight - sig.weight) > 0.001) {
        errors.push(`Weight mismatch for ${sig.validator.slice(0, 8)}...: claimed ${sig.weight}, trusted ${trustedValidator.weight}`);
        continue;
      }

      const publicKeyBytes = new Uint8Array(sig.publicKey);
      
      const expectedFingerprint = await computeFingerprint(publicKeyBytes);
      if (expectedFingerprint !== sig.validator) {
        errors.push(`Signature from ${sig.validator.slice(0, 8)}... has mismatched public key`);
        continue;
      }

      const trustedPubKeyMatch = trustedValidator.publicKey.every(
        (byte, i) => byte === sig.publicKey[i]
      ) && trustedValidator.publicKey.length === sig.publicKey.length;
      
      if (!trustedPubKeyMatch) {
        errors.push(`Public key mismatch for ${sig.validator.slice(0, 8)}...`);
        continue;
      }

      const signerSigningData = signingData + `:${sig.weight}`;
      const isValid = await verify(signerSigningData, sig.signature, publicKeyBytes);
      if (isValid) {
        validSignatures++;
        verifiedWeight += sig.weight;
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

  const trustedTotalWeight = trustedValidators.reduce((sum, v) => sum + v.weight, 0);
  
  if (Math.abs(trustedTotalWeight - proof.totalNetworkWeight) > 0.001 && trustedTotalWeight > 0) {
    errors.push(
      `Total network weight mismatch: proof claims ${proof.totalNetworkWeight}, trusted is ${trustedTotalWeight}`
    );
  }

  const computedWeightPercent = trustedTotalWeight > 0 
    ? (verifiedWeight / trustedTotalWeight) * 100 
    : 0;
    
  if (computedWeightPercent < config.minValidatorWeightPercent) {
    errors.push(
      `Insufficient verified weight: ${computedWeightPercent.toFixed(1)}% < ${config.minValidatorWeightPercent}% required`
    );
  }

  return {
    valid: errors.length === 0 && 
           validSignatures >= config.minSignaturesRequired && 
           computedWeightPercent >= config.minValidatorWeightPercent,
    checkpointId: proof.checkpointId,
    signatureCount: validSignatures,
    validatorWeightPercent: computedWeightPercent,
    errors
  };
}

export async function verifyCheckpointChain(
  proof: CheckpointProof,
  checkpointChain: CheckpointProof[],
  genesisConfig: GenesisConfig,
  config: CheckpointConfig = DEFAULT_CHECKPOINT_CONFIG
): Promise<CheckpointVerification> {
  if (proof.checkpointHeight === 0) {
    return verifyCheckpointProof(proof, genesisConfig.initialValidators, config);
  }

  let currentValidators = genesisConfig.initialValidators;
  
  const sortedChain = [...checkpointChain].sort((a, b) => a.checkpointHeight - b.checkpointHeight);
  
  for (const checkpoint of sortedChain) {
    if (checkpoint.checkpointHeight === 0) continue;
    
    const verification = await verifyCheckpointProof(checkpoint, currentValidators, config);
    if (!verification.valid) {
      return {
        valid: false,
        checkpointId: proof.checkpointId,
        signatureCount: 0,
        validatorWeightPercent: 0,
        errors: [`Chain broken at height ${checkpoint.checkpointHeight}: ${verification.errors.join(', ')}`]
      };
    }
    
    currentValidators = checkpoint.validators;
  }
  
  return verifyCheckpointProof(proof, currentValidators, config);
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
    p: proof.previousCheckpointId,
    a: proof.validators.map(v => ({
      a: v.address,
      p: v.publicKey,
      w: v.weight
    })),
    s: proof.signatures.map(sig => ({
      v: sig.validator,
      g: sig.signature,
      p: sig.publicKey,
      w: sig.weight,
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
      previousCheckpointId: compact.p || null,
      validators: (compact.a || []).map((v: any) => ({
        address: v.a,
        publicKey: v.p,
        weight: v.w
      })),
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
