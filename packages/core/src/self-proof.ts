import { deflate, inflate } from "pako";
import { sha256 } from "@noble/hashes/sha2.js";
import {
  verifyAggregatedSignature,
  parseBLSSignerBitmap,
  bytesToHex,
  hexToBytes,
} from "./bls.js";
import type {
  ValidatorEntry,
  Checkpoint,
  BLSCheckpointSignature,
  SignedTransaction,
} from "./types.js";
import { base64urlEncode, base64urlDecode } from "./encoding.js";

export interface ValidatorWitness {
  index: number;
  address: string;
  blsPublicKey: string; // base64url-encoded 96-byte G2 point
  weight: number;
}

export interface SelfContainedProof {
  version: number;
  txHash: string;
  txSignature: string;
  txFrom: string;
  txTo: string;
  txAmount: number;
  txNonce: number;
  txTimestamp: number;
  checkpointHeight: number;
  checkpointId: string;
  txMerkleRoot: string;
  stateRoot: string;
  receiptRoot: string;
  totalWeight: number;
  tipCount: number;
  merkleProof: string[];
  merkleIndex: number;
  blsAggregatedSig: string; // base64url-encoded 48-byte G1 signature
  blsSignerBitmap: string; // base64url-encoded bitmap
  blsSignerCount: number;
  validatorWitnesses: ValidatorWitness[];
  validatorSetRoot: string;
}

export interface ProofVerificationResult {
  valid: boolean;
  errors: string[];
  txHash: string;
  checkpointHeight: number;
  computedSignerWeight: number;
  totalWeight: number;
  signerCount: number;
  merkleVerified: boolean;
  blsVerified: boolean;
  validatorSetVerified: boolean;
}

const SELF_PROOF_VERSION = 3;

export function computeValidatorSetRoot(witnesses: ValidatorWitness[]): string {
  const sorted = [...witnesses].sort((a, b) => a.index - b.index);
  const entries = sorted.map(
    (w) =>
      `${w.index}:${w.address}:${w.blsPublicKey}:${w.weight}`,
  );
  const combined = entries.join("|");
  const hash = sha256(new TextEncoder().encode(combined));
  return bytesToHex(hash);
}

export function computeCheckpointSigningHash(
  checkpointId: string,
  height: number,
  txMerkleRoot: string,
  stateRoot: string,
  receiptRoot: string,
  totalWeight: number,
  tipCount: number,
  validatorSetRoot: string,
): Uint8Array {
  const signingData = `${checkpointId}:${height}:${txMerkleRoot}:${stateRoot}:${receiptRoot}:${totalWeight}:${tipCount}:${validatorSetRoot}`;
  return sha256(new TextEncoder().encode(signingData));
}

export function createSelfContainedProof(
  tx: SignedTransaction,
  checkpoint: Checkpoint,
  merkleProof: string[],
  merkleIndex: number,
): SelfContainedProof | null {
  if (!checkpoint.blsSignature) {
    return null;
  }
  if (!checkpoint.txMerkleRoot) {
    return null;
  }

  const blsSig = checkpoint.blsSignature;
  const signerIndices = parseBLSSignerBitmap(
    new Uint8Array(blsSig.signerBitmap),
    checkpoint.validators.length,
  );

  const validatorWitnesses: ValidatorWitness[] = [];

  for (let i = 0; i < checkpoint.validators.length; i++) {
    const v = checkpoint.validators[i];

    if (signerIndices.includes(i) && v.blsPublicKey) {
      validatorWitnesses.push({
        index: i,
        address: v.address,
        blsPublicKey: base64urlEncode(new Uint8Array(v.blsPublicKey)),
        weight: v.weight,
      });
    }
  }

  const validatorSetRoot = computeValidatorSetRoot(validatorWitnesses);

  return {
    version: SELF_PROOF_VERSION,
    txHash: tx.hash,
    txSignature: tx.sig,
    txFrom: tx.from,
    txTo: tx.to,
    txAmount: tx.amount,
    txNonce: tx.nonce,
    txTimestamp: tx.ts,
    checkpointHeight: checkpoint.height,
    checkpointId: checkpoint.checkpointId,
    txMerkleRoot: checkpoint.txMerkleRoot,
    stateRoot: checkpoint.stateRoot || "",
    receiptRoot: checkpoint.receiptRoot || "",
    totalWeight: checkpoint.totalWeight,
    tipCount: checkpoint.tipCount,
    merkleProof,
    merkleIndex,
    blsAggregatedSig: base64urlEncode(new Uint8Array(blsSig.aggregatedSignature)),
    blsSignerBitmap: base64urlEncode(new Uint8Array(blsSig.signerBitmap)),
    blsSignerCount: blsSig.signerCount,
    validatorWitnesses,
    validatorSetRoot,
  };
}

export function verifySelfContainedProof(
  proof: SelfContainedProof,
): ProofVerificationResult {
  let computedSignerWeight = 0;
  for (const w of proof.validatorWitnesses) {
    computedSignerWeight += w.weight;
  }

  const result: ProofVerificationResult = {
    valid: false,
    errors: [],
    txHash: proof.txHash,
    checkpointHeight: proof.checkpointHeight,
    computedSignerWeight,
    totalWeight: proof.totalWeight,
    signerCount: proof.blsSignerCount,
    merkleVerified: false,
    blsVerified: false,
    validatorSetVerified: false,
  };

  if (proof.version !== SELF_PROOF_VERSION) {
    result.errors.push(`Unsupported proof version: ${proof.version}`);
    return result;
  }

  const recomputedValidatorSetRoot = computeValidatorSetRoot(
    proof.validatorWitnesses,
  );

  if (recomputedValidatorSetRoot !== proof.validatorSetRoot) {
    result.errors.push(
      "Validator set root mismatch - validator data may have been tampered",
    );
    return result;
  }
  result.validatorSetVerified = true;

  try {
    const merkleValid = verifyMerkleProof(
      proof.txHash,
      proof.merkleProof,
      proof.merkleIndex,
      proof.txMerkleRoot,
    );
    result.merkleVerified = merkleValid;
    if (!merkleValid) {
      result.errors.push(
        "Merkle proof verification failed - tx not included in checkpoint",
      );
    }
  } catch (e: any) {
    result.errors.push(`Merkle verification error: ${e.message}`);
  }

  try {
    // binds validator weights (via validatorSetRoot) to the BLS signature, preventing weight forgery attacks
    const checkpointHash = computeCheckpointSigningHash(
      proof.checkpointId,
      proof.checkpointHeight,
      proof.txMerkleRoot,
      proof.stateRoot,
      proof.receiptRoot,
      proof.totalWeight,
      proof.tipCount,
      proof.validatorSetRoot,
    );

    const signerPubKeys = proof.validatorWitnesses.map(
      (w) => base64urlDecode(w.blsPublicKey),
    );

    const blsValid = verifyAggregatedSignature(
      checkpointHash,
      base64urlDecode(proof.blsAggregatedSig),
      signerPubKeys,
    );

    result.blsVerified = blsValid;
    if (!blsValid) {
      result.errors.push(
        "BLS signature verification failed - checkpoint signature invalid",
      );
    }
  } catch (e: any) {
    result.errors.push(`BLS verification error: ${e.message}`);
  }

  if (proof.totalWeight <= 0) {
    result.errors.push("Invalid total weight");
  } else {
    const weightRatio = computedSignerWeight / proof.totalWeight;
    if (weightRatio < 0.67) {
      result.errors.push(
        `Insufficient signer weight: ${(weightRatio * 100).toFixed(1)}% (need 67%)`,
      );
    }
  }

  result.valid =
    result.merkleVerified &&
    result.blsVerified &&
    result.validatorSetVerified &&
    result.errors.length === 0;
  return result;
}

function verifyMerkleProof(
  txHash: string,
  proof: string[],
  index: number,
  expectedRoot: string,
): boolean {
  let current = txHash;
  let idx = index;

  for (const sibling of proof) {
    const left = idx % 2 === 0 ? current : sibling;
    const right = idx % 2 === 0 ? sibling : current;
    const combined = left + right;
    const hash = sha256(new TextEncoder().encode(combined));
    current = bytesToHex(hash);
    idx = Math.floor(idx / 2);
  }

  return current === expectedRoot;
}

export function encodeSelfContainedProof(proof: SelfContainedProof): string {
  const json = JSON.stringify(proof);
  const compressed = deflate(new TextEncoder().encode(json), { level: 9 });
  return uint8ArrayToBase64Url(compressed);
}

export function decodeSelfContainedProof(encoded: string): SelfContainedProof {
  if (encoded.startsWith("rinku://sp/")) {
    encoded = encoded.slice(11);
  }
  const compressed = base64UrlToUint8Array(encoded);
  const json = new TextDecoder().decode(inflate(compressed));
  return JSON.parse(json);
}

export function createSelfProofURL(proof: SelfContainedProof): string {
  return `rinku://sp/${encodeSelfContainedProof(proof)}`;
}

function uint8ArrayToBase64Url(data: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < data.length; i++) {
    binary += String.fromCharCode(data[i]);
  }
  return btoa(binary)
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

function base64UrlToUint8Array(base64url: string): Uint8Array {
  let base64 = base64url.replace(/-/g, "+").replace(/_/g, "/");
  while (base64.length % 4) {
    base64 += "=";
  }
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

export function analyzeSelfProofSize(proof: SelfContainedProof): {
  jsonSize: number;
  compressedSize: number;
  base64Size: number;
  urlSize: number;
  qrViability: string;
} {
  const json = JSON.stringify(proof);
  const compressed = deflate(new TextEncoder().encode(json), { level: 9 });
  const base64 = uint8ArrayToBase64Url(compressed);
  const url = `rinku://sp/${base64}`;

  let qrViability = "Unknown";
  if (base64.length <= 395) qrViability = "v10 - Easy scan";
  else if (base64.length <= 758) qrViability = "v15 - Good";
  else if (base64.length <= 1249) qrViability = "v20 - Large QR";
  else if (base64.length <= 1853) qrViability = "v25 - Very large";
  else if (base64.length <= 2520) qrViability = "v30 - Huge";
  else if (base64.length <= 4296) qrViability = "v40 - Max QR";
  else qrViability = ">v40 - Too big for QR";

  return {
    jsonSize: json.length,
    compressedSize: compressed.length,
    base64Size: base64.length,
    urlSize: url.length,
    qrViability,
  };
}

export function getSelfProofSigningData(checkpoint: Checkpoint): string {
  return `${checkpoint.checkpointId}:${checkpoint.height}:${checkpoint.txMerkleRoot || ""}:${checkpoint.stateRoot || ""}:${checkpoint.receiptRoot || ""}:${checkpoint.totalWeight}:${checkpoint.tipCount}`;
}
