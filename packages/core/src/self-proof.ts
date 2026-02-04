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
import {
  buildMerkleSumTree,
  getMerkleSumProof,
  verifyMerkleSumProof,
  computeMerkleSumRootFromProofs,
  type MerkleSumLeaf,
  type MerkleSumProof,
  type MerkleSumRoot,
} from "./merkle-sum-tree.js";

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
  tipCount: number;
  merkleProof: string[];
  merkleIndex: number;
  blsAggregatedSig: string;
  blsSignerBitmap: string;
  blsSignerCount: number;
  signerMembershipProofs: MerkleSumProof[];
  validatorSumTreeRoot: MerkleSumRoot;
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

const SELF_PROOF_VERSION = 4;

export function validatorToMerkleSumLeaf(v: ValidatorEntry, index: number): MerkleSumLeaf {
  return {
    index,
    address: v.address,
    blsPublicKey: v.blsPublicKey ? base64urlEncode(new Uint8Array(v.blsPublicKey)) : "",
    weight: v.weight,
  };
}

export function computeValidatorSumTreeRoot(validators: ValidatorEntry[]): MerkleSumRoot {
  const leaves = validators.map((v, i) => validatorToMerkleSumLeaf(v, i));
  const { root } = buildMerkleSumTree(leaves);
  return root;
}

export function computeCheckpointSigningHash(
  checkpointId: string,
  height: number,
  txMerkleRoot: string,
  stateRoot: string,
  receiptRoot: string,
  tipCount: number,
  validatorSumTreeRoot: MerkleSumRoot,
): Uint8Array {
  const signingData = `${checkpointId}:${height}:${txMerkleRoot}:${stateRoot}:${receiptRoot}:${validatorSumTreeRoot.hash}:${validatorSumTreeRoot.totalWeight}:${tipCount}`;
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

  const leaves = checkpoint.validators.map((v, i) => validatorToMerkleSumLeaf(v, i));
  const { root } = buildMerkleSumTree(leaves);

  const signerMembershipProofs: MerkleSumProof[] = [];
  for (const idx of signerIndices) {
    const proof = getMerkleSumProof(leaves, idx);
    if (proof) {
      signerMembershipProofs.push(proof);
    }
  }

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
    tipCount: checkpoint.tipCount,
    merkleProof,
    merkleIndex,
    blsAggregatedSig: base64urlEncode(new Uint8Array(blsSig.aggregatedSignature)),
    blsSignerBitmap: base64urlEncode(new Uint8Array(blsSig.signerBitmap)),
    blsSignerCount: blsSig.signerCount,
    signerMembershipProofs,
    validatorSumTreeRoot: root,
  };
}

export function verifySelfContainedProof(
  proof: SelfContainedProof,
): ProofVerificationResult {
  let computedSignerWeight = 0;
  const totalWeight = proof.validatorSumTreeRoot.totalWeight;

  const result: ProofVerificationResult = {
    valid: false,
    errors: [],
    txHash: proof.txHash,
    checkpointHeight: proof.checkpointHeight,
    computedSignerWeight: 0,
    totalWeight,
    signerCount: proof.blsSignerCount,
    merkleVerified: false,
    blsVerified: false,
    validatorSetVerified: false,
  };

  if (proof.version !== SELF_PROOF_VERSION) {
    result.errors.push(`Unsupported proof version: ${proof.version}`);
    return result;
  }

  const computedRoot = computeMerkleSumRootFromProofs(proof.signerMembershipProofs);
  if (!computedRoot) {
    result.errors.push("Failed to compute root from membership proofs - inconsistent proofs");
    return result;
  }

  if (computedRoot.hash !== proof.validatorSumTreeRoot.hash) {
    result.errors.push(
      `Validator sum tree root hash mismatch: computed ${computedRoot.hash.slice(0, 16)}..., expected ${proof.validatorSumTreeRoot.hash.slice(0, 16)}...`
    );
    return result;
  }

  if (computedRoot.totalWeight !== proof.validatorSumTreeRoot.totalWeight) {
    result.errors.push(
      `Validator sum tree total weight mismatch: computed ${computedRoot.totalWeight}, expected ${proof.validatorSumTreeRoot.totalWeight}`
    );
    return result;
  }

  for (const membershipProof of proof.signerMembershipProofs) {
    const verifyResult = verifyMerkleSumProof(membershipProof, proof.validatorSumTreeRoot);
    if (!verifyResult.valid) {
      result.errors.push(`Invalid membership proof for validator ${membershipProof.leaf.index}: ${verifyResult.errors.join(", ")}`);
      return result;
    }
    computedSignerWeight += verifyResult.leafWeight;
  }

  result.computedSignerWeight = computedSignerWeight;
  result.validatorSetVerified = true;

  try {
    const txMerkleValid = verifyTxMerkleProof(
      proof.txHash,
      proof.merkleProof,
      proof.merkleIndex,
      proof.txMerkleRoot,
    );
    result.merkleVerified = txMerkleValid;
    if (!txMerkleValid) {
      result.errors.push(
        "Merkle proof verification failed - tx not included in checkpoint",
      );
    }
  } catch (e: any) {
    result.errors.push(`Merkle verification error: ${e.message}`);
  }

  try {
    const checkpointHash = computeCheckpointSigningHash(
      proof.checkpointId,
      proof.checkpointHeight,
      proof.txMerkleRoot,
      proof.stateRoot,
      proof.receiptRoot,
      proof.tipCount,
      proof.validatorSumTreeRoot,
    );

    const signerPubKeys = proof.signerMembershipProofs.map(
      (p) => base64urlDecode(p.leaf.blsPublicKey),
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

  if (totalWeight <= 0) {
    result.errors.push("Invalid total weight");
  } else {
    const weightRatio = computedSignerWeight / totalWeight;
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

function verifyTxMerkleProof(
  txHash: string,
  proof: string[],
  index: number,
  expectedRoot: string,
): boolean {
  let current = txHash;
  let idx = BigInt(index);

  for (const sibling of proof) {
    const left = idx % 2n === 0n ? current : sibling;
    const right = idx % 2n === 0n ? sibling : current;
    const combined = left + right;
    const hash = sha256(new TextEncoder().encode(combined));
    current = bytesToHex(hash);
    idx = idx / 2n;
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
  const root = computeValidatorSumTreeRoot(checkpoint.validators);
  return `${checkpoint.checkpointId}:${checkpoint.height}:${checkpoint.txMerkleRoot || ""}:${checkpoint.stateRoot || ""}:${checkpoint.receiptRoot || ""}:${root.hash}:${root.totalWeight}:${checkpoint.tipCount}`;
}

export { type MerkleSumProof, type MerkleSumRoot, type MerkleSumLeaf };
