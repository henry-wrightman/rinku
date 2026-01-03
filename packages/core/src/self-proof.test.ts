import { describe, it, expect } from 'vitest';
import {
  createSelfContainedProof,
  verifySelfContainedProof,
  encodeSelfContainedProof,
  decodeSelfContainedProof,
  createSelfProofURL,
  analyzeSelfProofSize,
  computeValidatorSetRoot,
  computeCheckpointSigningHash,
  type SelfContainedProof,
  type ValidatorWitness
} from './self-proof.js';
import { blsSign, aggregateSignatures, createSignerBitmap, bytesToHex, generateBLSKeyPair } from './bls.js';
import { sha256 } from '@noble/hashes/sha2.js';
import { base64urlEncode, base64urlDecode } from './encoding.js';
import type { SignedTransaction, Checkpoint, BLSCheckpointSignature } from './types.js';

function uint8ToBase64url(data: Uint8Array): string {
  return base64urlEncode(data);
}

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

function createMockCheckpointWithBLS(validators: Array<{ address: string; blsPublicKey: Uint8Array; weight: number }>, blsSig: BLSCheckpointSignature): Checkpoint {
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

describe('Self-Contained Proof', () => {
  describe('createSelfContainedProof', () => {
    it('should create a valid self-contained proof', () => {
      const key1 = generateBLSKeyPair();
      const key2 = generateBLSKeyPair();
      const key3 = generateBLSKeyPair();

      const validators = [
        { address: 'val1', blsPublicKey: key1.publicKey, weight: 100 },
        { address: 'val2', blsPublicKey: key2.publicKey, weight: 100 },
        { address: 'val3', blsPublicKey: key3.publicKey, weight: 100 }
      ];

      const witnesses: ValidatorWitness[] = validators.map((v, i) => ({
        index: i,
        address: v.address,
        blsPublicKey: uint8ToBase64url(v.blsPublicKey),
        weight: v.weight
      }));
      const validatorSetRoot = computeValidatorSetRoot(witnesses);

      const checkpointHash = computeCheckpointSigningHash(
        'cp-test-12345',
        1,
        'abc123def456789012345678901234567890123456789012345678901234abcd',
        'state-root-hash',
        'receipt-root-hash',
        300,
        5,
        validatorSetRoot
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
        validatorSetRoot
      };

      const checkpoint = createMockCheckpointWithBLS(validators, blsSig);
      const tx = createMockTransaction();
      tx.hash = 'abc123def456789012345678901234567890123456789012345678901234abcd';

      const proof = createSelfContainedProof(tx, checkpoint, [], 0);

      expect(proof).not.toBeNull();
      expect(proof!.version).toBe(3);
      expect(proof!.txHash).toBe(tx.hash);
      expect(proof!.checkpointHeight).toBe(1);
      expect(proof!.blsSignerCount).toBe(3);
      expect(proof!.validatorWitnesses.length).toBe(3);
      expect(proof!.totalWeight).toBe(300);
      expect(proof!.stateRoot).toBe('state-root-hash');
      expect(proof!.receiptRoot).toBe('receipt-root-hash');
      expect(typeof proof!.blsAggregatedSig).toBe('string');
      expect(typeof proof!.blsSignerBitmap).toBe('string');
      expect(typeof proof!.validatorWitnesses[0].blsPublicKey).toBe('string');
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
    it('should verify a valid proof with correct BLS signatures', () => {
      const key1 = generateBLSKeyPair();
      const key2 = generateBLSKeyPair();
      const key3 = generateBLSKeyPair();

      const txHash = 'abc123def456789012345678901234567890123456789012345678901234abcd';
      const stateRoot = 'state-root-hash';
      const receiptRoot = 'receipt-root-hash';
      const totalWeight = 300;
      const tipCount = 5;

      const witnesses: ValidatorWitness[] = [
        { index: 0, address: 'val1', blsPublicKey: uint8ToBase64url(key1.publicKey), weight: 100 },
        { index: 1, address: 'val2', blsPublicKey: uint8ToBase64url(key2.publicKey), weight: 100 },
        { index: 2, address: 'val3', blsPublicKey: uint8ToBase64url(key3.publicKey), weight: 100 }
      ];
      const validatorSetRoot = computeValidatorSetRoot(witnesses);

      const checkpointHash = computeCheckpointSigningHash(
        'cp-test-12345',
        1,
        txHash,
        stateRoot,
        receiptRoot,
        totalWeight,
        tipCount,
        validatorSetRoot
      );

      const sig1 = blsSign(checkpointHash, key1.privateKey);
      const sig2 = blsSign(checkpointHash, key2.privateKey);
      const sig3 = blsSign(checkpointHash, key3.privateKey);

      const aggregatedSig = aggregateSignatures([sig1, sig2, sig3]);
      const bitmap = createSignerBitmap([0, 1, 2], 3);

      const proof: SelfContainedProof = {
        version: 3,
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
        totalWeight,
        tipCount,
        merkleProof: [],
        merkleIndex: 0,
        blsAggregatedSig: uint8ToBase64url(aggregatedSig),
        blsSignerBitmap: uint8ToBase64url(bitmap),
        blsSignerCount: 3,
        validatorWitnesses: witnesses,
        validatorSetRoot
      };

      const result = verifySelfContainedProof(proof);

      expect(result.blsVerified).toBe(true);
      expect(result.merkleVerified).toBe(true);
      expect(result.validatorSetVerified).toBe(true);
      expect(result.valid).toBe(true);
      expect(result.errors).toHaveLength(0);
      expect(result.computedSignerWeight).toBe(300);
    });

    it('should reject proof with tampered validator set', () => {
      const key1 = generateBLSKeyPair();
      const fakeTamperedRoot = 'tampered-root-hash-that-does-not-match';

      const proof: SelfContainedProof = {
        version: 3,
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
        totalWeight: 100,
        tipCount: 1,
        merkleProof: [],
        merkleIndex: 0,
        blsAggregatedSig: uint8ToBase64url(new Uint8Array(48)),
        blsSignerBitmap: uint8ToBase64url(new Uint8Array([1])),
        blsSignerCount: 1,
        validatorWitnesses: [{ index: 0, address: 'val1', blsPublicKey: uint8ToBase64url(key1.publicKey), weight: 100 }],
        validatorSetRoot: fakeTamperedRoot
      };

      const result = verifySelfContainedProof(proof);

      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Validator set root mismatch'))).toBe(true);
    });

    it('should reject proof with insufficient weight', () => {
      const key1 = generateBLSKeyPair();

      const witnesses: ValidatorWitness[] = [
        { index: 0, address: 'val1', blsPublicKey: uint8ToBase64url(key1.publicKey), weight: 50 }
      ];
      const validatorSetRoot = computeValidatorSetRoot(witnesses);

      const proof: SelfContainedProof = {
        version: 3,
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
        totalWeight: 100,
        tipCount: 1,
        merkleProof: [],
        merkleIndex: 0,
        blsAggregatedSig: uint8ToBase64url(new Uint8Array(48)),
        blsSignerBitmap: uint8ToBase64url(new Uint8Array([1])),
        blsSignerCount: 1,
        validatorWitnesses: witnesses,
        validatorSetRoot
      };

      const result = verifySelfContainedProof(proof);

      expect(result.valid).toBe(false);
      expect(result.errors.some(e => e.includes('Insufficient signer weight'))).toBe(true);
    });
  });

  describe('Encoding/Decoding', () => {
    it('should encode and decode proof correctly', () => {
      const key1 = generateBLSKeyPair();
      const witnesses: ValidatorWitness[] = [
        { index: 0, address: 'val1', blsPublicKey: uint8ToBase64url(key1.publicKey), weight: 100 }
      ];
      const validatorSetRoot = computeValidatorSetRoot(witnesses);

      const proof: SelfContainedProof = {
        version: 3,
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
        totalWeight: 100,
        tipCount: 5,
        merkleProof: ['proof1', 'proof2'],
        merkleIndex: 3,
        blsAggregatedSig: uint8ToBase64url(new Uint8Array([1, 2, 3, 4])),
        blsSignerBitmap: uint8ToBase64url(new Uint8Array([5])),
        blsSignerCount: 1,
        validatorWitnesses: witnesses,
        validatorSetRoot
      };

      const encoded = encodeSelfContainedProof(proof);
      const decoded = decodeSelfContainedProof(encoded);

      expect(decoded.version).toBe(proof.version);
      expect(decoded.txHash).toBe(proof.txHash);
      expect(decoded.checkpointHeight).toBe(proof.checkpointHeight);
      expect(decoded.blsSignerCount).toBe(proof.blsSignerCount);
      expect(decoded.stateRoot).toBe(proof.stateRoot);
      expect(decoded.totalWeight).toBe(proof.totalWeight);
    });

    it('should create URL format', () => {
      const key1 = generateBLSKeyPair();
      const witnesses: ValidatorWitness[] = [
        { index: 0, address: 'val1', blsPublicKey: uint8ToBase64url(key1.publicKey), weight: 100 }
      ];
      const validatorSetRoot = computeValidatorSetRoot(witnesses);

      const proof: SelfContainedProof = {
        version: 3,
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
        totalWeight: 100,
        tipCount: 5,
        merkleProof: [],
        merkleIndex: 0,
        blsAggregatedSig: uint8ToBase64url(new Uint8Array([1, 2, 3])),
        blsSignerBitmap: uint8ToBase64url(new Uint8Array([1])),
        blsSignerCount: 1,
        validatorWitnesses: witnesses,
        validatorSetRoot
      };

      const url = createSelfProofURL(proof);

      expect(url.startsWith('rinku://sp/')).toBe(true);

      const decoded = decodeSelfContainedProof(url);
      expect(decoded.txHash).toBe(proof.txHash);
    });

    it('should analyze proof size for QR viability', () => {
      const key1 = generateBLSKeyPair();
      const witnesses: ValidatorWitness[] = [
        { index: 0, address: 'val1', blsPublicKey: uint8ToBase64url(key1.publicKey), weight: 100 }
      ];
      const validatorSetRoot = computeValidatorSetRoot(witnesses);

      const proof: SelfContainedProof = {
        version: 3,
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
        totalWeight: 100,
        tipCount: 5,
        merkleProof: [],
        merkleIndex: 0,
        blsAggregatedSig: uint8ToBase64url(new Uint8Array(48)),
        blsSignerBitmap: uint8ToBase64url(new Uint8Array([1])),
        blsSignerCount: 1,
        validatorWitnesses: witnesses,
        validatorSetRoot
      };

      const sizeAnalysis = analyzeSelfProofSize(proof);

      expect(sizeAnalysis.jsonSize).toBeGreaterThan(0);
      expect(sizeAnalysis.compressedSize).toBeGreaterThan(0);
      expect(sizeAnalysis.base64Size).toBeGreaterThan(0);
      expect(sizeAnalysis.urlSize).toBeGreaterThan(0);
      expect(sizeAnalysis.qrViability).toBeTruthy();
    });
  });
});
