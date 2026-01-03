import { bls12_381 as bls } from '@noble/curves/bls12-381.js';
import { sha256 } from '@noble/hashes/sha2.js';
import { bytesToHex, hexToBytes } from '@noble/hashes/utils.js';

const sigs = bls.shortSignatures;
const G1 = bls.G1;
const G2 = bls.G2;

export interface BLSKeyPair {
  publicKey: Uint8Array;
  privateKey: Uint8Array;
  fingerprint: string;
}

export interface AggregatedSignature {
  signature: Uint8Array;
  signerBitmap: Uint8Array;
  signerCount: number;
}

export function generateBLSKeyPair(): BLSKeyPair {
  const secretKey = bls.utils.randomSecretKey();
  const publicKeyPoint = sigs.getPublicKey(secretKey);
  const pubKeyBytes = publicKeyPoint.toBytes(true);
  const fingerprint = computeBLSFingerprint(pubKeyBytes);
  
  return {
    publicKey: pubKeyBytes,
    privateKey: secretKey,
    fingerprint
  };
}

export function computeBLSFingerprint(publicKey: Uint8Array): string {
  const hash = sha256(publicKey);
  return bytesToHex(hash).slice(0, 40);
}

export function blsGetPublicKey(privateKey: Uint8Array): Uint8Array {
  const pubKeyPoint = sigs.getPublicKey(privateKey);
  return pubKeyPoint.toBytes(true);
}

export function blsSign(message: Uint8Array, privateKey: Uint8Array): Uint8Array {
  const msgPoint = sigs.hash(message);
  const sigPoint = sigs.sign(msgPoint, privateKey);
  return sigs.Signature.toBytes(sigPoint);
}

export function blsVerify(
  message: Uint8Array,
  signature: Uint8Array,
  publicKey: Uint8Array
): boolean {
  try {
    const msgPoint = sigs.hash(message);
    return sigs.verify(signature, msgPoint, publicKey);
  } catch {
    return false;
  }
}

export function aggregateSignatures(signatures: Uint8Array[]): Uint8Array {
  if (signatures.length === 0) {
    throw new Error('No signatures to aggregate');
  }
  const aggPoint = sigs.aggregateSignatures(signatures);
  return sigs.Signature.toBytes(aggPoint);
}

export function aggregatePublicKeys(publicKeys: Uint8Array[]): Uint8Array {
  if (publicKeys.length === 0) {
    throw new Error('No public keys to aggregate');
  }
  const aggPoint = sigs.aggregatePublicKeys(publicKeys);
  return aggPoint.toBytes(true);
}

export function verifyAggregatedSignature(
  message: Uint8Array,
  aggregatedSig: Uint8Array,
  publicKeys: Uint8Array[]
): boolean {
  try {
    const aggPubKey = sigs.aggregatePublicKeys(publicKeys);
    const msgPoint = sigs.hash(message);
    return sigs.verify(aggregatedSig, msgPoint, aggPubKey.toBytes(true));
  } catch {
    return false;
  }
}

export function createSignerBitmap(
  signerIndices: number[],
  totalValidators: number
): Uint8Array {
  const byteCount = Math.ceil(totalValidators / 8);
  const bitmap = new Uint8Array(byteCount);
  
  for (const idx of signerIndices) {
    if (idx >= 0 && idx < totalValidators) {
      const byteIdx = Math.floor(idx / 8);
      const bitIdx = idx % 8;
      bitmap[byteIdx] |= (1 << bitIdx);
    }
  }
  
  return bitmap;
}

export function parseBLSSignerBitmap(
  bitmap: Uint8Array,
  totalValidators: number
): number[] {
  const signerIndices: number[] = [];
  
  for (let i = 0; i < totalValidators; i++) {
    const byteIdx = Math.floor(i / 8);
    const bitIdx = i % 8;
    
    if (byteIdx < bitmap.length && (bitmap[byteIdx] & (1 << bitIdx)) !== 0) {
      signerIndices.push(i);
    }
  }
  
  return signerIndices;
}

export function createAggregatedCheckpointSignature(
  checkpointHash: Uint8Array,
  validators: Array<{ index: number; privateKey: Uint8Array; publicKey: Uint8Array }>
): {
  aggregatedSig: Uint8Array;
  signerBitmap: Uint8Array;
  signerCount: number;
} {
  const signatures: Uint8Array[] = [];
  const signerIndices: number[] = [];
  
  for (const validator of validators) {
    const sig = blsSign(checkpointHash, validator.privateKey);
    signatures.push(sig);
    signerIndices.push(validator.index);
  }
  
  const aggregatedSig = aggregateSignatures(signatures);
  const maxIndex = Math.max(...signerIndices) + 1;
  const signerBitmap = createSignerBitmap(signerIndices, maxIndex);
  
  return {
    aggregatedSig,
    signerBitmap,
    signerCount: validators.length
  };
}

export function verifyAggregatedCheckpointSignature(
  checkpointHash: Uint8Array,
  aggregatedSig: Uint8Array,
  signerBitmap: Uint8Array,
  validatorPublicKeys: Uint8Array[]
): boolean {
  try {
    const signerIndices = parseBLSSignerBitmap(signerBitmap, validatorPublicKeys.length);
    
    if (signerIndices.length === 0) {
      return false;
    }
    
    const signerPubKeys = signerIndices.map(i => validatorPublicKeys[i]);
    return verifyAggregatedSignature(checkpointHash, aggregatedSig, signerPubKeys);
  } catch {
    return false;
  }
}

export function serializeBLSKeyPair(keyPair: BLSKeyPair): string {
  return JSON.stringify({
    publicKey: bytesToHex(keyPair.publicKey),
    privateKey: bytesToHex(keyPair.privateKey),
    fingerprint: keyPair.fingerprint
  });
}

export function deserializeBLSKeyPair(data: string): BLSKeyPair {
  const parsed = JSON.parse(data);
  return {
    publicKey: hexToBytes(parsed.publicKey),
    privateKey: hexToBytes(parsed.privateKey),
    fingerprint: parsed.fingerprint
  };
}

export function getBLSSignatureSize(): { signature: number; publicKey: number } {
  return {
    signature: 48,
    publicKey: 96
  };
}

export { bytesToHex, hexToBytes };
