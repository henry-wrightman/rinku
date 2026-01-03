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
  previousCheckpointId: string | null,
  txMerkleRoot?: string,
  txHashes?: string[],
  stateRoot?: string,
  receiptRoot?: string
): Promise<Checkpoint> {
  const timestamp = Date.now();
  return {
    checkpointId: await createCheckpointId(height, merkleRoot, timestamp),
    height,
    merkleRoot,
    txMerkleRoot,
    txHashes,
    stateRoot,
    receiptRoot,
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

export function getBLSCheckpointSigningData(
  checkpoint: Checkpoint,
  validatorSumTreeRootHash: string,
  validatorSumTreeTotalWeight: number
): string {
  return `${checkpoint.checkpointId}:${checkpoint.height}:${checkpoint.txMerkleRoot || ''}:${checkpoint.stateRoot || ''}:${checkpoint.receiptRoot || ''}:${validatorSumTreeRootHash}:${validatorSumTreeTotalWeight}:${checkpoint.tipCount}`;
}

export function createCheckpointProof(
  checkpoint: Checkpoint,
  validatorWeightPercent: number
): CheckpointProof {
  return {
    checkpointId: checkpoint.checkpointId,
    checkpointHeight: checkpoint.height,
    merkleRoot: checkpoint.merkleRoot,
    txMerkleRoot: checkpoint.txMerkleRoot,
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

// ============================================
// Profile B Signature Formats
// ============================================

/** 
 * Compact signature for Profile B proofs.
 * Trades validator identity for size reduction.
 */
export interface CompactValidatorSignature {
  idx: number;           // Index into checkpoint's validator array
  sig: string;           // Signature (88 chars for ECDSA P-256)
}

/** 
 * Profile B checkpoint certificate - compact finality proof.
 * Uses indexed signatures for size efficiency.
 */
export interface CheckpointCertificate {
  checkpointId: string;
  merkleRoot: string;
  stateRoot: string;
  receiptRoot: string;
  height: number;
  validatorSetHash: string;
  totalWeight: number;
  signerBitmap: string;      // Hex bitmap of which validators signed (bit i = validator[i])
  signatures: CompactValidatorSignature[];
}

/** Create a checkpoint certificate from a full checkpoint */
export function createCheckpointCertificate(
  checkpoint: Checkpoint,
  stateRoot: string,
  receiptRoot: string
): CheckpointCertificate {
  const validatorMap = new Map(
    checkpoint.validators.map((v, i) => [v.address, i])
  );
  
  const signerBitmap = createSignerBitmap(
    checkpoint.signatures.map(s => s.validator),
    validatorMap
  );
  
  const compactSigs: CompactValidatorSignature[] = checkpoint.signatures.map(s => ({
    idx: validatorMap.get(s.validator) ?? -1,
    sig: s.signature
  })).filter(s => s.idx >= 0);
  
  return {
    checkpointId: checkpoint.checkpointId,
    merkleRoot: checkpoint.merkleRoot,
    stateRoot,
    receiptRoot,
    height: checkpoint.height,
    validatorSetHash: checkpoint.validatorSetHash,
    totalWeight: checkpoint.totalWeight,
    signerBitmap,
    signatures: compactSigs
  };
}

/** Create a hex bitmap of which validators signed */
function createSignerBitmap(
  signerAddresses: string[],
  validatorMap: Map<string, number>
): string {
  const signerSet = new Set(signerAddresses);
  const numValidators = validatorMap.size;
  const numBytes = Math.ceil(numValidators / 8);
  const bytes = new Uint8Array(numBytes);
  
  for (const [address, idx] of validatorMap) {
    if (signerSet.has(address)) {
      const byteIdx = Math.floor(idx / 8);
      const bitIdx = idx % 8;
      bytes[byteIdx] |= (1 << bitIdx);
    }
  }
  
  return Array.from(bytes)
    .map(b => b.toString(16).padStart(2, '0'))
    .join('');
}

/** Parse signer bitmap to get list of validator indices */
export function parseSignerBitmap(bitmap: string): number[] {
  const indices: number[] = [];
  const bytes = bitmap.match(/.{2}/g) || [];
  
  for (let byteIdx = 0; byteIdx < bytes.length; byteIdx++) {
    const byte = parseInt(bytes[byteIdx], 16);
    for (let bitIdx = 0; bitIdx < 8; bitIdx++) {
      if (byte & (1 << bitIdx)) {
        indices.push(byteIdx * 8 + bitIdx);
      }
    }
  }
  
  return indices;
}

/** Verify a checkpoint certificate */
export async function verifyCheckpointCertificate(
  cert: CheckpointCertificate,
  validators: ValidatorEntry[],
  config: CheckpointConfig = DEFAULT_CHECKPOINT_CONFIG
): Promise<{ valid: boolean; errors: string[]; weightPercent: number }> {
  const errors: string[] = [];
  let verifiedWeight = 0;
  const totalWeight = validators.reduce((sum, v) => sum + v.weight, 0);
  
  const signerIndices = parseSignerBitmap(cert.signerBitmap);
  
  if (signerIndices.length !== cert.signatures.length) {
    errors.push(`Bitmap/signature count mismatch: ${signerIndices.length} vs ${cert.signatures.length}`);
  }
  
  const signingData = JSON.stringify({
    checkpointId: cert.checkpointId,
    height: cert.height,
    merkleRoot: cert.merkleRoot,
    stateRoot: cert.stateRoot,
    receiptRoot: cert.receiptRoot,
    totalWeight: cert.totalWeight,
    validatorSetHash: cert.validatorSetHash
  });
  
  for (const compactSig of cert.signatures) {
    if (compactSig.idx < 0 || compactSig.idx >= validators.length) {
      errors.push(`Invalid validator index: ${compactSig.idx}`);
      continue;
    }
    
    const validator = validators[compactSig.idx];
    const publicKeyBytes = new Uint8Array(validator.publicKey);
    
    const signerSigningData = signingData + `:${validator.weight}`;
    
    try {
      const isValid = await verify(signerSigningData, compactSig.sig, publicKeyBytes);
      if (isValid) {
        verifiedWeight += validator.weight;
      } else {
        errors.push(`Invalid signature from validator ${compactSig.idx}`);
      }
    } catch (err) {
      errors.push(`Failed to verify signature from validator ${compactSig.idx}`);
    }
  }
  
  const weightPercent = totalWeight > 0 ? (verifiedWeight / totalWeight) * 100 : 0;
  
  if (weightPercent < config.minValidatorWeightPercent) {
    errors.push(`Insufficient weight: ${weightPercent.toFixed(1)}% < ${config.minValidatorWeightPercent}% required`);
  }
  
  return {
    valid: errors.length === 0 && weightPercent >= config.minValidatorWeightPercent,
    errors,
    weightPercent
  };
}

/** Encode checkpoint certificate for URL embedding */
export function encodeCheckpointCertificate(cert: CheckpointCertificate): string {
  const compact = {
    c: cert.checkpointId,
    m: cert.merkleRoot,
    s: cert.stateRoot,
    r: cert.receiptRoot,
    h: cert.height,
    v: cert.validatorSetHash,
    w: cert.totalWeight,
    b: cert.signerBitmap,
    g: cert.signatures.map(s => `${s.idx}:${s.sig}`)
  };
  return Buffer.from(JSON.stringify(compact)).toString('base64url');
}

/** Decode checkpoint certificate from URL */
export function decodeCheckpointCertificate(encoded: string): CheckpointCertificate | null {
  try {
    const json = Buffer.from(encoded, 'base64url').toString('utf-8');
    const compact = JSON.parse(json);
    return {
      checkpointId: compact.c,
      merkleRoot: compact.m,
      stateRoot: compact.s,
      receiptRoot: compact.r,
      height: compact.h,
      validatorSetHash: compact.v,
      totalWeight: compact.w,
      signerBitmap: compact.b,
      signatures: compact.g.map((s: string) => {
        const [idx, sig] = s.split(':');
        return { idx: parseInt(idx), sig };
      })
    };
  } catch {
    return null;
  }
}

/** Estimate URL size for different signature approaches */
export function estimateProfileBSize(
  numValidators: number,
  numSigners: number,
  avgReceiptSize: number = 400
): {
  fullSignatures: number;      // All sigs with full validator info
  compactSignatures: number;   // Indexed sigs with bitmap
  perSignatureBytes: number;
} {
  const validatorEntrySize = 50;      // Address + weight
  const fullSignatureSize = 130;      // Validator address + sig + weight + pubkey ref
  const compactSignatureSize = 92;    // Index (2) + signature (88)
  const bitmapSize = Math.ceil(numValidators / 8) * 2;  // Hex encoding
  
  const fullSignatures = 
    avgReceiptSize + 
    (numValidators * validatorEntrySize) + 
    (numSigners * fullSignatureSize);
    
  const compactSignatures = 
    avgReceiptSize + 
    bitmapSize + 
    (numSigners * compactSignatureSize);
  
  return {
    fullSignatures,
    compactSignatures,
    perSignatureBytes: compactSignatureSize
  };
}
