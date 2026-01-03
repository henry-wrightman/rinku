import { describe, it, expect } from 'vitest';
import {
  createSelfContainedProof,
  verifySelfContainedProof,
  encodeSelfContainedProof,
  decodeSelfContainedProof,
  createSelfProofURL,
  analyzeSelfProofSize,
  computeCheckpointSigningHash,
  computeValidatorSumTreeRoot,
  validatorToMerkleSumLeaf,
  type SelfContainedProof,
} from './self-proof.js';
import { blsSign, aggregateSignatures, createSignerBitmap, generateBLSKeyPair } from './bls.js';
import { buildMerkleSumTree, getMerkleSumProof, type MerkleSumLeaf, type MerkleSumRoot, type MerkleSumProof } from './merkle-sum-tree.js';
import { base64urlEncode } from './encoding.js';
import type { SignedTransaction, Checkpoint, BLSCheckpointSignature, ValidatorEntry } from './types.js';

function createMockTransaction(): SignedTransaction {
  return {
    hash: 'abc123def456789012345678901234567890123456789012345678901234abcd',
    from: 'sender-address-123',
    to: 'recipient-address-456',
    amount: 100,
    fee: 0.001,
    nonce: 1,
    ts: Date.now(),
    sig: 'mock-signature-data',
    tipUrls: []
  };
}

function createMockCheckpointWithBLS(
  validators: Array<{ address: string; blsPublicKey: Uint8Array; weight: number }>,
  blsSig: BLSCheckpointSignature
): Checkpoint {
  return {
    checkpointId: 'cp-test-12345',
    height: 1,
    previousCheckpointId: null,
    timestamp: Date.now(),
    merkleRoot: 'merkle-root-hash',
    txMerkleRoot: 'abc123def456789012345678901234567890123456789012345678901234abcd',
    stateRoot: 'state-root-hash',
    receiptRoot: 'receipt-root-hash',
    tipCount: 5,
    totalTransactions: 10,
    validatorSetHash: 'validator-set-hash',
    validators: validators.map((v, i) => ({
      address: v.address,
      publicKey: [],
      blsPublicKey: Array.from(v.blsPublicKey),
      weight: v.weight
    })),
    totalWeight: validators.reduce((sum, v) => sum + v.weight, 0),
    signatures: [],
    blsSignature: blsSig
  };
}

describe('Self-Contained Proof v4 (MerkleSumTree)', () => {
  describe('createSelfContainedProof', () => {
    it('should create a valid self-contained proof with MerkleSumTree', () => {
      const key1 = generateBLSKeyPair();
      const key2 = generateBLSKeyPair();
      const key3 = generateBLSKeyPair();

      const validators = [
        { address: 'val1', blsPublicKey: key1.publicKey, weight: 100 },
        { address: 'val2', blsPublicKey: key2.publicKey, weight: 100 },
        { address: 'val3', blsPublicKey: key3.publicKey, weight: 100 }
      ];

      const validatorEntries: ValidatorEntry[] = validators.map((v, i) => ({
        address: v.address,
        publicKey: [],
        blsPublicKey: Array.from(v.blsPublicKey),
        weight: v.weight
      }));

      const validatorSumTreeRoot = computeValidatorSumTreeRoot(validatorEntries);

      const checkpointHash = computeCheckpointSigningHash(
        'cp-test-12345',
        1,
        'abc123def456789012345678901234567890123456789012345678901234abcd',
        'state-root-hash',
        'receipt-root-hash',
        5,
        validatorSumTreeRoot
      );

      const sig1 = blsSign(checkpointHash, key1.privateKey);
      const sig2 = blsSign(checkpointHash, key2.privateKey);
      const sig3 = blsSign(checkpointHash, key3.privateKey);

      const aggregatedSig = aggregateSignatures([sig1, sig2, sig3]);
      const bitmap = createSignerBitmap([0, 1, 2], 3);

      const blsSig: BLSCheckpointSignature = {
        aggregatedSignature: Array.from(aggregatedSig),
        signerBitmap: Array.from(bitmap),
        signerCount: 3,
        validatorSetRoot: validatorSumTreeRoot.hash
      };

      const checkpoint = createMockCheckpointWithBLS(validators, blsSig);
      const tx = createMockTransaction();

      const proof = createSelfContainedProof(tx, checkpoint, [], 0);

      expect(proof).not.toBeNull();
      expect(proof!.version).toBe(4);
      expect(proof!.txHash).toBe(tx.hash);
      expect(proof!.checkpointHeight).toBe(1);
      expect(proof!.blsSignerCount).toBe(3);
      expect(proof!.signerMembershipProofs.length).toBe(3);
      expect(proof!.validatorSumTreeRoot.totalWeight).toBe(300);
      expect(proof!.stateRoot).toBe('state-root-hash');
      expect(proof!.receiptRoot).toBe('receipt-root-hash');
      expect(typeof proof!.blsAggregatedSig).toBe('string');
    });

    it('should return null if checkpoint has no BLS signature', () => {
      const checkpoint: Checkpoint = {
        checkpointId: 'cp-test',
        height: 1,
        previousCheckpointId: null,
        timestamp: Date.now(),
        merkleRoot: 'merkle-root',
        txMerkleRoot: 'root-hash',
        stateRoot: 'state-root',
        receiptRoot: 'receipt-root',
        tipCount: 1,
        totalTransactions: 1,
        validatorSetHash: 'validator-hash',
        validators: [],
        totalWeight: 100,
        signatures: []
      };

      const tx = createMockTransaction();
      const proof = createSelfContainedProof(tx, checkpoint, [], 0);

      expect(proof).toBeNull();
    });
  });

  describe('verifySelfContainedProof', () => {
    it('should verify a valid proof with MerkleSumTree membership proofs', () => {
      const key1 = generateBLSKeyPair();
      const key2 = generateBLSKeyPair();
      const key3 = generateBLSKeyPair();

      const txHash = 'abc123def456789012345678901234567890123456789012345678901234abcd';
      const stateRoot = 'state-root-hash';
      const receiptRoot = 'receipt-root-hash';
      const tipCount = 5;

      const leaves: MerkleSumLeaf[] = [
        { index: 0, address: 'val1', blsPublicKey: base64urlEncode(key1.publicKey), weight: 100 },
        { index: 1, address: 'val2', blsPublicKey: base64urlEncode(key2.publicKey), weight: 100 },
        { index: 2, address: 'val3', blsPublicKey: base64urlEncode(key3.publicKey), weight: 100 }
      ];

      const { root } = buildMerkleSumTree(leaves);
      const signerMembershipProofs = [0, 1, 2].map(i => getMerkleSumProof(leaves, i)!);

      const checkpointHash = computeCheckpointSigningHash(
        'cp-test-12345',
        1,
        txHash,
        stateRoot,
        receiptRoot,
        tipCount,
        root
      );

      const sig1 = blsSign(checkpointHash, key1.privateKey);
      const sig2 = blsSign(checkpointHash, key2.privateKey);
      const sig3 = blsSign(checkpointHash, key3.privateKey);

      const aggregatedSig = aggregateSignatures([sig1, sig2, sig3]);
      const bitmap = createSignerBitmap([0, 1, 2], 3);

      const proof: SelfContainedProof = {
        version: 4,
        txHash,
        txSignature: 'mock-sig',
        txFrom: 'sender',
        txTo: 'recipient',
        txAmount: 100,
        txNonce: 1,
        txTimestamp: Date.now(),
        checkpointHeight: 1,
        checkpointId: 'cp-test-12345',
        txMerkleRoot: txHash,
        stateRoot,
        receiptRoot,
        tipCount,
        merkleProof: [],
        merkleIndex: 0,
        blsAggregatedSig: base64urlEncode(aggregatedSig),
        blsSignerBitmap: base64urlEncode(bitmap),
        blsSignerCount: 3,
        signerMembershipProofs,
        validatorSumTreeRoot: root
      };

      const result = verifySelfContainedProof(proof);

      expect(result.blsVerified).toBe(true);
      expect(result.merkleVerified).toBe(true);
      expect(result.validatorSetVerified).toBe(true);
      expect(result.valid).toBe(true);
      expect(result.errors).toHaveLength(0);
      expect(result.computedSignerWeight).toBe(300);
      expect(result.totalWeight).toBe(300);
    });

    it('should reject proof with tampered totalWeight (denominator attack)', () => {
      const key1 = generateBLSKeyPair();

      const leaves: MerkleSumLeaf[] = [
        { index: 0, address: 'val1', blsPublicKey: base64urlEncode(key1.publicKey), weight: 100 }
      ];

      const { root } = buildMerkleSumTree(leaves);
      const signerMembershipProofs = [getMerkleSumProof(leaves, 0)!];

      const tamperedRoot: MerkleSumRoot = {
        hash: root.hash,
        totalWeight: 50
      };

      const proof: SelfContainedProof = {
        version: 4,
        txHash: 'abc123',
        txSignature: 'sig',
        txFrom: 'sender',
        txTo: 'recipient',
        txAmount: 100,
        txNonce: 1,
        txTimestamp: Date.now(),
        checkpointHeight: 1,
        checkpointId: 'cp-1',
        txMerkleRoot: 'root',
        stateRoot: 'state',
        receiptRoot: 'receipt',
        tipCount: 1,
        merkleProof: [],
        merkleIndex: 0,
        blsAggregatedSig: base64urlEncode(new Uint8Array(48)),
        blsSignerBitmap: base64urlEncode(new Uint8Array([1])),
        blsSignerCount: 1,
        signerMembershipProofs,
        validatorSumTreeRoot: tamperedRoot
      };

      const result = verifySelfContainedProof(proof);

      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('total weight mismatch'))).toBe(true);
    });

    it('should reject proof with tampered signer weight', () => {
      const key1 = generateBLSKeyPair();

      const leaves: MerkleSumLeaf[] = [
        { index: 0, address: 'val1', blsPublicKey: base64urlEncode(key1.publicKey), weight: 100 }
      ];

      const { root } = buildMerkleSumTree(leaves);
      const proof1 = getMerkleSumProof(leaves, 0)!;
      proof1.leaf.weight = 99999;

      const proof: SelfContainedProof = {
        version: 4,
        txHash: 'abc123',
        txSignature: 'sig',
        txFrom: 'sender',
        txTo: 'recipient',
        txAmount: 100,
        txNonce: 1,
        txTimestamp: Date.now(),
        checkpointHeight: 1,
        checkpointId: 'cp-1',
        txMerkleRoot: 'root',
        stateRoot: 'state',
        receiptRoot: 'receipt',
        tipCount: 1,
        merkleProof: [],
        merkleIndex: 0,
        blsAggregatedSig: base64urlEncode(new Uint8Array(48)),
        blsSignerBitmap: base64urlEncode(new Uint8Array([1])),
        blsSignerCount: 1,
        signerMembershipProofs: [proof1],
        validatorSumTreeRoot: root
      };

      const result = verifySelfContainedProof(proof);

      expect(result.valid).toBe(false);
    });

    it('should reject proof with insufficient weight', () => {
      const key1 = generateBLSKeyPair();
      const key2 = generateBLSKeyPair();
      const key3 = generateBLSKeyPair();

      const leaves: MerkleSumLeaf[] = [
        { index: 0, address: 'val1', blsPublicKey: base64urlEncode(key1.publicKey), weight: 100 },
        { index: 1, address: 'val2', blsPublicKey: base64urlEncode(key2.publicKey), weight: 100 },
        { index: 2, address: 'val3', blsPublicKey: base64urlEncode(key3.publicKey), weight: 100 }
      ];

      const { root } = buildMerkleSumTree(leaves);
      const signerMembershipProofs = [getMerkleSumProof(leaves, 0)!];

      const checkpointHash = computeCheckpointSigningHash(
        'cp-1', 1, 'root', 'state', 'receipt', 1, root
      );

      const sig1 = blsSign(checkpointHash, key1.privateKey);
      const aggregatedSig = aggregateSignatures([sig1]);

      const proof: SelfContainedProof = {
        version: 4,
        txHash: 'root',
        txSignature: 'sig',
        txFrom: 'sender',
        txTo: 'recipient',
        txAmount: 100,
        txNonce: 1,
        txTimestamp: Date.now(),
        checkpointHeight: 1,
        checkpointId: 'cp-1',
        txMerkleRoot: 'root',
        stateRoot: 'state',
        receiptRoot: 'receipt',
        tipCount: 1,
        merkleProof: [],
        merkleIndex: 0,
        blsAggregatedSig: base64urlEncode(aggregatedSig),
        blsSignerBitmap: base64urlEncode(new Uint8Array([1])),
        blsSignerCount: 1,
        signerMembershipProofs,
        validatorSumTreeRoot: root
      };

      const result = verifySelfContainedProof(proof);

      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Insufficient signer weight'))).toBe(true);
      expect(result.computedSignerWeight).toBe(100);
      expect(result.totalWeight).toBe(300);
    });
  });

  describe('Encoding/Decoding', () => {
    it('should encode and decode proof correctly', () => {
      const key1 = generateBLSKeyPair();

      const leaves: MerkleSumLeaf[] = [
        { index: 0, address: 'val1', blsPublicKey: base64urlEncode(key1.publicKey), weight: 100 }
      ];

      const { root } = buildMerkleSumTree(leaves);
      const signerMembershipProofs = [getMerkleSumProof(leaves, 0)!];

      const proof: SelfContainedProof = {
        version: 4,
        txHash: 'abc123',
        txSignature: 'sig',
        txFrom: 'sender',
        txTo: 'recipient',
        txAmount: 100,
        txNonce: 1,
        txTimestamp: 1234567890,
        checkpointHeight: 1,
        checkpointId: 'cp-1',
        txMerkleRoot: 'root',
        stateRoot: 'state',
        receiptRoot: 'receipt',
        tipCount: 5,
        merkleProof: ['proof1', 'proof2'],
        merkleIndex: 3,
        blsAggregatedSig: base64urlEncode(new Uint8Array([1, 2, 3, 4])),
        blsSignerBitmap: base64urlEncode(new Uint8Array([5])),
        blsSignerCount: 1,
        signerMembershipProofs,
        validatorSumTreeRoot: root
      };

      const encoded = encodeSelfContainedProof(proof);
      const decoded = decodeSelfContainedProof(encoded);

      expect(decoded.version).toBe(proof.version);
      expect(decoded.txHash).toBe(proof.txHash);
      expect(decoded.checkpointHeight).toBe(proof.checkpointHeight);
      expect(decoded.blsSignerCount).toBe(proof.blsSignerCount);
      expect(decoded.stateRoot).toBe(proof.stateRoot);
      expect(decoded.validatorSumTreeRoot.totalWeight).toBe(proof.validatorSumTreeRoot.totalWeight);
    });

    it('should create URL format', () => {
      const key1 = generateBLSKeyPair();

      const leaves: MerkleSumLeaf[] = [
        { index: 0, address: 'val1', blsPublicKey: base64urlEncode(key1.publicKey), weight: 100 }
      ];

      const { root } = buildMerkleSumTree(leaves);
      const signerMembershipProofs = [getMerkleSumProof(leaves, 0)!];

      const proof: SelfContainedProof = {
        version: 4,
        txHash: 'abc123',
        txSignature: 'sig',
        txFrom: 'sender',
        txTo: 'recipient',
        txAmount: 100,
        txNonce: 1,
        txTimestamp: 1234567890,
        checkpointHeight: 1,
        checkpointId: 'cp-1',
        txMerkleRoot: 'root',
        stateRoot: 'state',
        receiptRoot: 'receipt',
        tipCount: 5,
        merkleProof: [],
        merkleIndex: 0,
        blsAggregatedSig: base64urlEncode(new Uint8Array([1, 2, 3])),
        blsSignerBitmap: base64urlEncode(new Uint8Array([1])),
        blsSignerCount: 1,
        signerMembershipProofs,
        validatorSumTreeRoot: root
      };

      const url = createSelfProofURL(proof);

      expect(url.startsWith('rinku://sp/')).toBe(true);

      const decoded = decodeSelfContainedProof(url);
      expect(decoded.txHash).toBe(proof.txHash);
    });

    it('should analyze proof size for QR viability', () => {
      const key1 = generateBLSKeyPair();

      const leaves: MerkleSumLeaf[] = [
        { index: 0, address: 'val1', blsPublicKey: base64urlEncode(key1.publicKey), weight: 100 }
      ];

      const { root } = buildMerkleSumTree(leaves);
      const signerMembershipProofs = [getMerkleSumProof(leaves, 0)!];

      const proof: SelfContainedProof = {
        version: 4,
        txHash: 'abc123',
        txSignature: 'sig',
        txFrom: 'sender',
        txTo: 'recipient',
        txAmount: 100,
        txNonce: 1,
        txTimestamp: 1234567890,
        checkpointHeight: 1,
        checkpointId: 'cp-1',
        txMerkleRoot: 'root',
        stateRoot: 'state',
        receiptRoot: 'receipt',
        tipCount: 5,
        merkleProof: [],
        merkleIndex: 0,
        blsAggregatedSig: base64urlEncode(new Uint8Array(48)),
        blsSignerBitmap: base64urlEncode(new Uint8Array([1])),
        blsSignerCount: 1,
        signerMembershipProofs,
        validatorSumTreeRoot: root
      };

      const sizeAnalysis = analyzeSelfProofSize(proof);

      expect(sizeAnalysis.jsonSize).toBeGreaterThan(0);
      expect(sizeAnalysis.compressedSize).toBeGreaterThan(0);
      expect(sizeAnalysis.base64Size).toBeGreaterThan(0);
      expect(sizeAnalysis.urlSize).toBeGreaterThan(0);
      expect(sizeAnalysis.qrViability).toBeTruthy();
    });
  });

  describe('Security: Denominator Attack Prevention', () => {
    it('should derive totalWeight from MerkleSumTree root, not trust claimed value', () => {
      const key1 = generateBLSKeyPair();
      const key2 = generateBLSKeyPair();

      const leaves: MerkleSumLeaf[] = [
        { index: 0, address: 'val1', blsPublicKey: base64urlEncode(key1.publicKey), weight: 100 },
        { index: 1, address: 'val2', blsPublicKey: base64urlEncode(key2.publicKey), weight: 200 }
      ];

      const { root } = buildMerkleSumTree(leaves);
      
      expect(root.totalWeight).toBe(300);

      const signerMembershipProofs = [getMerkleSumProof(leaves, 0)!];

      const checkpointHash = computeCheckpointSigningHash(
        'cp-1', 1, 'hash', 'state', 'receipt', 1, root
      );

      const attackerRoot: MerkleSumRoot = {
        hash: root.hash,
        totalWeight: 100
      };

      const proof: SelfContainedProof = {
        version: 4,
        txHash: 'hash',
        txSignature: 'sig',
        txFrom: 'sender',
        txTo: 'recipient',
        txAmount: 100,
        txNonce: 1,
        txTimestamp: Date.now(),
        checkpointHeight: 1,
        checkpointId: 'cp-1',
        txMerkleRoot: 'hash',
        stateRoot: 'state',
        receiptRoot: 'receipt',
        tipCount: 1,
        merkleProof: [],
        merkleIndex: 0,
        blsAggregatedSig: base64urlEncode(new Uint8Array(48)),
        blsSignerBitmap: base64urlEncode(new Uint8Array([1])),
        blsSignerCount: 1,
        signerMembershipProofs,
        validatorSumTreeRoot: attackerRoot
      };

      const result = verifySelfContainedProof(proof);

      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('total weight mismatch'))).toBe(true);
    });
  });
});
