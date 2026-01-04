import { describe, it, expect } from 'vitest';
import {
  generateBLSKeyPair,
  computeBLSFingerprint,
  blsGetPublicKey,
  blsSign,
  blsVerify,
  aggregateSignatures,
  aggregatePublicKeys,
  verifyAggregatedSignature,
  createSignerBitmap,
  parseBLSSignerBitmap,
  createAggregatedCheckpointSignature,
  verifyAggregatedCheckpointSignature,
  serializeBLSKeyPair,
  deserializeBLSKeyPair,
  getBLSSignatureSize,
  bytesToHex,
  hexToBytes,
} from '../bls.js';

describe('BLS Module', () => {
  describe('Key Generation', () => {
    it('should generate valid key pair', () => {
      const keyPair = generateBLSKeyPair();
      expect(keyPair.publicKey).toBeInstanceOf(Uint8Array);
      expect(keyPair.privateKey).toBeInstanceOf(Uint8Array);
      expect(keyPair.fingerprint).toBeDefined();
      expect(keyPair.fingerprint.length).toBe(40);
    });

    it('should generate unique key pairs', () => {
      const kp1 = generateBLSKeyPair();
      const kp2 = generateBLSKeyPair();
      expect(bytesToHex(kp1.privateKey)).not.toBe(bytesToHex(kp2.privateKey));
    });

    it('should compute fingerprint from public key', () => {
      const keyPair = generateBLSKeyPair();
      const fingerprint = computeBLSFingerprint(keyPair.publicKey);
      expect(fingerprint).toBe(keyPair.fingerprint);
    });

    it('should derive public key from private key', () => {
      const keyPair = generateBLSKeyPair();
      const derivedPubKey = blsGetPublicKey(keyPair.privateKey);
      expect(bytesToHex(derivedPubKey)).toBe(bytesToHex(keyPair.publicKey));
    });
  });

  describe('Signing and Verification', () => {
    it('should sign and verify message', () => {
      const keyPair = generateBLSKeyPair();
      const message = new TextEncoder().encode('Hello, BLS!');
      const signature = blsSign(message, keyPair.privateKey);
      expect(signature).toBeInstanceOf(Uint8Array);
      expect(signature.length).toBe(48);
      const isValid = blsVerify(message, signature, keyPair.publicKey);
      expect(isValid).toBe(true);
    });

    it('should reject invalid signature', () => {
      const keyPair = generateBLSKeyPair();
      const message = new TextEncoder().encode('Hello, BLS!');
      const signature = blsSign(message, keyPair.privateKey);
      const wrongMessage = new TextEncoder().encode('Wrong message');
      const isValid = blsVerify(wrongMessage, signature, keyPair.publicKey);
      expect(isValid).toBe(false);
    });

    it('should reject signature with wrong public key', () => {
      const kp1 = generateBLSKeyPair();
      const kp2 = generateBLSKeyPair();
      const message = new TextEncoder().encode('Test');
      const signature = blsSign(message, kp1.privateKey);
      const isValid = blsVerify(message, signature, kp2.publicKey);
      expect(isValid).toBe(false);
    });

    it('should handle malformed signature gracefully', () => {
      const keyPair = generateBLSKeyPair();
      const message = new TextEncoder().encode('Test');
      const isValid = blsVerify(message, new Uint8Array(10), keyPair.publicKey);
      expect(isValid).toBe(false);
    });
  });

  describe('Signature Aggregation', () => {
    it('should aggregate multiple signatures', () => {
      const kp1 = generateBLSKeyPair();
      const kp2 = generateBLSKeyPair();
      const message = new TextEncoder().encode('Aggregate me');
      const sig1 = blsSign(message, kp1.privateKey);
      const sig2 = blsSign(message, kp2.privateKey);
      const aggregated = aggregateSignatures([sig1, sig2]);
      expect(aggregated).toBeInstanceOf(Uint8Array);
      expect(aggregated.length).toBe(48);
    });

    it('should throw when aggregating empty signature list', () => {
      expect(() => aggregateSignatures([])).toThrow('No signatures to aggregate');
    });

    it('should aggregate single signature', () => {
      const keyPair = generateBLSKeyPair();
      const message = new TextEncoder().encode('Single');
      const sig = blsSign(message, keyPair.privateKey);
      const aggregated = aggregateSignatures([sig]);
      expect(aggregated).toBeInstanceOf(Uint8Array);
    });
  });

  describe('Public Key Aggregation', () => {
    it('should aggregate multiple public keys', () => {
      const kp1 = generateBLSKeyPair();
      const kp2 = generateBLSKeyPair();
      const aggregated = aggregatePublicKeys([kp1.publicKey, kp2.publicKey]);
      expect(aggregated).toBeInstanceOf(Uint8Array);
    });

    it('should throw when aggregating empty public key list', () => {
      expect(() => aggregatePublicKeys([])).toThrow('No public keys to aggregate');
    });
  });

  describe('Aggregated Signature Verification', () => {
    it('should verify aggregated signature from multiple signers', () => {
      const kp1 = generateBLSKeyPair();
      const kp2 = generateBLSKeyPair();
      const kp3 = generateBLSKeyPair();
      const message = new TextEncoder().encode('Multi-signer message');
      const sig1 = blsSign(message, kp1.privateKey);
      const sig2 = blsSign(message, kp2.privateKey);
      const sig3 = blsSign(message, kp3.privateKey);
      const aggregatedSig = aggregateSignatures([sig1, sig2, sig3]);
      const isValid = verifyAggregatedSignature(
        message,
        aggregatedSig,
        [kp1.publicKey, kp2.publicKey, kp3.publicKey]
      );
      expect(isValid).toBe(true);
    });

    it('should reject aggregated signature with missing signer', () => {
      const kp1 = generateBLSKeyPair();
      const kp2 = generateBLSKeyPair();
      const kp3 = generateBLSKeyPair();
      const message = new TextEncoder().encode('Test');
      const sig1 = blsSign(message, kp1.privateKey);
      const sig2 = blsSign(message, kp2.privateKey);
      const aggregatedSig = aggregateSignatures([sig1, sig2]);
      const isValid = verifyAggregatedSignature(
        message,
        aggregatedSig,
        [kp1.publicKey, kp2.publicKey, kp3.publicKey]
      );
      expect(isValid).toBe(false);
    });

    it('should handle verification error gracefully', () => {
      const keyPair = generateBLSKeyPair();
      const message = new TextEncoder().encode('Test');
      const isValid = verifyAggregatedSignature(
        message,
        new Uint8Array(10),
        [keyPair.publicKey]
      );
      expect(isValid).toBe(false);
    });
  });

  describe('Signer Bitmap', () => {
    it('should create bitmap for signer indices', () => {
      const bitmap = createSignerBitmap([0, 2, 5], 8);
      expect(bitmap.length).toBe(1);
      expect(bitmap[0]).toBe(0b00100101);
    });

    it('should handle large validator sets', () => {
      const bitmap = createSignerBitmap([0, 15, 16], 20);
      expect(bitmap.length).toBe(3);
      expect(bitmap[0]).toBe(0b00000001);
      expect(bitmap[1]).toBe(0b10000000);
      expect(bitmap[2]).toBe(0b00000001);
    });

    it('should ignore out-of-range indices', () => {
      const bitmap = createSignerBitmap([0, 1, 100], 10);
      expect(bitmap.length).toBe(2);
      expect(parseBLSSignerBitmap(bitmap, 10)).toEqual([0, 1]);
    });

    it('should parse bitmap back to indices', () => {
      const indices = [1, 3, 7, 10];
      const bitmap = createSignerBitmap(indices, 15);
      const parsed = parseBLSSignerBitmap(bitmap, 15);
      expect(parsed).toEqual(indices);
    });

    it('should handle empty signer list', () => {
      const bitmap = createSignerBitmap([], 8);
      expect(bitmap.length).toBe(1);
      expect(parseBLSSignerBitmap(bitmap, 8)).toEqual([]);
    });
  });

  describe('Checkpoint Signature', () => {
    it('should create and verify aggregated checkpoint signature', () => {
      const validators = [
        { ...generateBLSKeyPair(), index: 0 },
        { ...generateBLSKeyPair(), index: 1 },
        { ...generateBLSKeyPair(), index: 2 },
      ];
      const checkpointHash = new TextEncoder().encode('checkpoint:1:abc123');
      const result = createAggregatedCheckpointSignature(checkpointHash, validators);
      expect(result.aggregatedSig).toBeInstanceOf(Uint8Array);
      expect(result.signerBitmap).toBeInstanceOf(Uint8Array);
      expect(result.signerCount).toBe(3);
      const publicKeys = validators.map(v => v.publicKey);
      const isValid = verifyAggregatedCheckpointSignature(
        checkpointHash,
        result.aggregatedSig,
        result.signerBitmap,
        publicKeys
      );
      expect(isValid).toBe(true);
    });

    it('should verify with subset of validators', () => {
      const validators = [
        { ...generateBLSKeyPair(), index: 0 },
        { ...generateBLSKeyPair(), index: 1 },
        { ...generateBLSKeyPair(), index: 2 },
      ];
      const checkpointHash = new TextEncoder().encode('checkpoint:2:def456');
      const signingValidators = [validators[0], validators[2]];
      const result = createAggregatedCheckpointSignature(checkpointHash, signingValidators);
      const allPublicKeys = validators.map(v => v.publicKey);
      const isValid = verifyAggregatedCheckpointSignature(
        checkpointHash,
        result.aggregatedSig,
        result.signerBitmap,
        allPublicKeys
      );
      expect(isValid).toBe(true);
    });

    it('should reject invalid checkpoint signature', () => {
      const validators = [{ ...generateBLSKeyPair(), index: 0 }];
      const checkpointHash = new TextEncoder().encode('checkpoint:1');
      const result = createAggregatedCheckpointSignature(checkpointHash, validators);
      const wrongHash = new TextEncoder().encode('wrong:checkpoint');
      const isValid = verifyAggregatedCheckpointSignature(
        wrongHash,
        result.aggregatedSig,
        result.signerBitmap,
        validators.map(v => v.publicKey)
      );
      expect(isValid).toBe(false);
    });

    it('should return false for empty signer bitmap', () => {
      const checkpointHash = new TextEncoder().encode('checkpoint:1');
      const publicKeys = [generateBLSKeyPair().publicKey];
      const isValid = verifyAggregatedCheckpointSignature(
        checkpointHash,
        new Uint8Array(48),
        new Uint8Array(1),
        publicKeys
      );
      expect(isValid).toBe(false);
    });

    it('should handle verification errors gracefully', () => {
      const checkpointHash = new TextEncoder().encode('checkpoint:1');
      const publicKeys = [generateBLSKeyPair().publicKey];
      const isValid = verifyAggregatedCheckpointSignature(
        checkpointHash,
        new Uint8Array(10),
        new Uint8Array([0b00000001]),
        publicKeys
      );
      expect(isValid).toBe(false);
    });
  });

  describe('Key Serialization', () => {
    it('should serialize and deserialize key pair', () => {
      const original = generateBLSKeyPair();
      const serialized = serializeBLSKeyPair(original);
      expect(typeof serialized).toBe('string');
      const deserialized = deserializeBLSKeyPair(serialized);
      expect(bytesToHex(deserialized.publicKey)).toBe(bytesToHex(original.publicKey));
      expect(bytesToHex(deserialized.privateKey)).toBe(bytesToHex(original.privateKey));
      expect(deserialized.fingerprint).toBe(original.fingerprint);
    });

    it('should produce valid JSON', () => {
      const keyPair = generateBLSKeyPair();
      const serialized = serializeBLSKeyPair(keyPair);
      expect(() => JSON.parse(serialized)).not.toThrow();
    });
  });

  describe('Signature Size', () => {
    it('should return correct sizes', () => {
      const sizes = getBLSSignatureSize();
      expect(sizes.signature).toBe(48);
      expect(sizes.publicKey).toBe(96);
    });
  });

  describe('Hex Utils', () => {
    it('should convert bytes to hex', () => {
      const bytes = new Uint8Array([0, 15, 255]);
      const hex = bytesToHex(bytes);
      expect(hex).toBe('000fff');
    });

    it('should convert hex to bytes', () => {
      const bytes = hexToBytes('0a1b2c');
      expect(Array.from(bytes)).toEqual([10, 27, 44]);
    });

    it('should round-trip bytes through hex', () => {
      const original = new Uint8Array([1, 2, 3, 128, 255]);
      const hex = bytesToHex(original);
      const restored = hexToBytes(hex);
      expect(Array.from(restored)).toEqual(Array.from(original));
    });
  });
});
