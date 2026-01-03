import { deflate, inflate } from 'pako';
import {
  blsSign,
  blsVerify,
  aggregateSignatures,
  verifyAggregatedSignature,
  createSignerBitmap,
  parseBLSSignerBitmap,
  bytesToHex,
  hexToBytes
} from './bls.js';

export interface CompactProof {
  version: number;
  txHash: Uint8Array;
  txSignature: Uint8Array;
  checkpointHeight: number;
  merkleProof: Uint8Array[];
  merkleIndex: number;
  aggregatedValidatorSig: Uint8Array;
  signerBitmap: Uint8Array;
  validatorSetRoot: Uint8Array;
}

export interface EncodedProof {
  binary: Uint8Array;
  base64url: string;
  url: string;
}

const PROOF_VERSION = 1;

function writeVarInt(value: number): Uint8Array {
  const bytes: number[] = [];
  while (value > 0x7f) {
    bytes.push((value & 0x7f) | 0x80);
    value >>>= 7;
  }
  bytes.push(value & 0x7f);
  return new Uint8Array(bytes);
}

function readVarInt(data: Uint8Array, offset: number): { value: number; bytesRead: number } {
  let value = 0;
  let shift = 0;
  let bytesRead = 0;
  
  while (offset + bytesRead < data.length) {
    const byte = data[offset + bytesRead];
    value |= (byte & 0x7f) << shift;
    bytesRead++;
    if ((byte & 0x80) === 0) break;
    shift += 7;
  }
  
  return { value, bytesRead };
}

export function encodeCompactProof(proof: CompactProof): EncodedProof {
  const parts: Uint8Array[] = [];
  
  parts.push(new Uint8Array([PROOF_VERSION]));
  
  parts.push(proof.txHash);
  
  parts.push(proof.txSignature);
  
  parts.push(writeVarInt(proof.checkpointHeight));
  
  parts.push(writeVarInt(proof.merkleProof.length));
  for (const hash of proof.merkleProof) {
    parts.push(hash);
  }
  
  parts.push(writeVarInt(proof.merkleIndex));
  
  parts.push(proof.aggregatedValidatorSig);
  
  parts.push(writeVarInt(proof.signerBitmap.length));
  parts.push(proof.signerBitmap);
  
  parts.push(proof.validatorSetRoot);
  
  const totalLength = parts.reduce((sum, p) => sum + p.length, 0);
  const binary = new Uint8Array(totalLength);
  let offset = 0;
  for (const part of parts) {
    binary.set(part, offset);
    offset += part.length;
  }
  
  const compressed = deflate(binary, { level: 9 });
  
  const base64url = uint8ArrayToBase64Url(compressed);
  
  const url = `rinku://p/${base64url}`;
  
  return {
    binary: compressed,
    base64url,
    url
  };
}

export function decodeCompactProof(encoded: string | Uint8Array): CompactProof {
  let compressed: Uint8Array;
  
  if (typeof encoded === 'string') {
    if (encoded.startsWith('rinku://p/')) {
      encoded = encoded.slice(10);
    }
    compressed = base64UrlToUint8Array(encoded);
  } else {
    compressed = encoded;
  }
  
  const binary = inflate(compressed);
  let offset = 0;
  
  const version = binary[offset++];
  if (version !== PROOF_VERSION) {
    throw new Error(`Unsupported proof version: ${version}`);
  }
  
  const txHash = binary.slice(offset, offset + 32);
  offset += 32;
  
  const txSignature = binary.slice(offset, offset + 64);
  offset += 64;
  
  const { value: checkpointHeight, bytesRead: cpBytes } = readVarInt(binary, offset);
  offset += cpBytes;
  
  const { value: merkleDepth, bytesRead: mdBytes } = readVarInt(binary, offset);
  offset += mdBytes;
  
  const merkleProof: Uint8Array[] = [];
  for (let i = 0; i < merkleDepth; i++) {
    merkleProof.push(binary.slice(offset, offset + 32));
    offset += 32;
  }
  
  const { value: merkleIndex, bytesRead: miBytes } = readVarInt(binary, offset);
  offset += miBytes;
  
  const aggregatedValidatorSig = binary.slice(offset, offset + 48);
  offset += 48;
  
  const { value: bitmapLength, bytesRead: blBytes } = readVarInt(binary, offset);
  offset += blBytes;
  
  const signerBitmap = binary.slice(offset, offset + bitmapLength);
  offset += bitmapLength;
  
  const validatorSetRoot = binary.slice(offset, offset + 32);
  
  return {
    version,
    txHash,
    txSignature,
    checkpointHeight,
    merkleProof,
    merkleIndex,
    aggregatedValidatorSig,
    signerBitmap,
    validatorSetRoot
  };
}

export function uint8ArrayToBase64Url(data: Uint8Array): string {
  let binary = '';
  for (let i = 0; i < data.length; i++) {
    binary += String.fromCharCode(data[i]);
  }
  return btoa(binary)
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');
}

export function base64UrlToUint8Array(base64url: string): Uint8Array {
  let base64 = base64url.replace(/-/g, '+').replace(/_/g, '/');
  while (base64.length % 4) {
    base64 += '=';
  }
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

export interface ProofSizeAnalysis {
  rawBytes: number;
  compressedBytes: number;
  base64Chars: number;
  qrVersion: string;
  viability: string;
}

export function analyzeProofSize(proof: CompactProof): ProofSizeAnalysis {
  const rawSize = 
    1 + 
    32 + 
    64 + 
    4 + 
    1 + (proof.merkleProof.length * 32) +
    2 + 
    48 + 
    1 + proof.signerBitmap.length +
    32;
  
  const encoded = encodeCompactProof(proof);
  
  let qrVersion = 'N/A';
  let viability = 'Unknown';
  
  const chars = encoded.base64url.length;
  
  if (chars <= 395) { qrVersion = 'v10'; viability = 'Easy scan'; }
  else if (chars <= 758) { qrVersion = 'v15'; viability = 'Good'; }
  else if (chars <= 1249) { qrVersion = 'v20'; viability = 'Large QR'; }
  else if (chars <= 1853) { qrVersion = 'v25'; viability = 'Very large'; }
  else if (chars <= 2520) { qrVersion = 'v30'; viability = 'Huge'; }
  else if (chars <= 4296) { qrVersion = 'v40'; viability = 'Max QR'; }
  else { qrVersion = '>v40'; viability = 'Too big'; }
  
  return {
    rawBytes: rawSize,
    compressedBytes: encoded.binary.length,
    base64Chars: chars,
    qrVersion,
    viability
  };
}

export function createMockProof(
  validatorCount: number,
  merkleDepth: number = 10
): CompactProof {
  const txHash = new Uint8Array(32);
  crypto.getRandomValues(txHash);
  
  const txSignature = new Uint8Array(64);
  crypto.getRandomValues(txSignature);
  
  const merkleProof: Uint8Array[] = [];
  for (let i = 0; i < merkleDepth; i++) {
    const hash = new Uint8Array(32);
    crypto.getRandomValues(hash);
    merkleProof.push(hash);
  }
  
  const aggregatedValidatorSig = new Uint8Array(48);
  crypto.getRandomValues(aggregatedValidatorSig);
  
  const signerBitmap = new Uint8Array(Math.ceil(validatorCount / 8));
  for (let i = 0; i < validatorCount; i++) {
    const byteIdx = Math.floor(i / 8);
    const bitIdx = i % 8;
    signerBitmap[byteIdx] |= (1 << bitIdx);
  }
  
  const validatorSetRoot = new Uint8Array(32);
  crypto.getRandomValues(validatorSetRoot);
  
  return {
    version: PROOF_VERSION,
    txHash,
    txSignature,
    checkpointHeight: 12345,
    merkleProof,
    merkleIndex: 42,
    aggregatedValidatorSig,
    signerBitmap,
    validatorSetRoot
  };
}
