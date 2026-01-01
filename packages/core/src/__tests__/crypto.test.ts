import { describe, it, expect } from 'vitest';
import {
  generateKeyPair,
  sign,
  verify,
  computeFingerprint,
  serializeKeyPair,
  deserializeKeyPair,
  hash,
} from '../crypto.js';

describe('Crypto Module', () => {
  describe('Key Generation', () => {
    it('should generate valid key pairs', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      
      expect(publicKey).toBeInstanceOf(Uint8Array);
      expect(privateKey).toBeInstanceOf(Uint8Array);
      expect(publicKey.length).toBeGreaterThan(0);
    });

    it('should generate unique key pairs', async () => {
      const kp1 = await generateKeyPair();
      const kp2 = await generateKeyPair();
      
      const fp1 = await computeFingerprint(kp1.publicKey);
      const fp2 = await computeFingerprint(kp2.publicKey);
      
      expect(fp1).not.toBe(fp2);
    });
  });

  describe('Hash', () => {
    it('should compute deterministic hashes', async () => {
      const hash1 = await hash('Hello, Rinku!');
      const hash2 = await hash('Hello, Rinku!');
      
      expect(hash1).toBe(hash2);
    });

    it('should produce different hashes for different inputs', async () => {
      const hash1 = await hash('input1');
      const hash2 = await hash('input2');
      
      expect(hash1).not.toBe(hash2);
    });
  });

  describe('Fingerprint', () => {
    it('should compute deterministic fingerprints', async () => {
      const { publicKey } = await generateKeyPair();
      
      const fp1 = await computeFingerprint(publicKey);
      const fp2 = await computeFingerprint(publicKey);
      
      expect(fp1).toBe(fp2);
      expect(fp1.length).toBeGreaterThan(0);
    });

    it('should produce different fingerprints for different keys', async () => {
      const kp1 = await generateKeyPair();
      const kp2 = await generateKeyPair();
      
      const fp1 = await computeFingerprint(kp1.publicKey);
      const fp2 = await computeFingerprint(kp2.publicKey);
      
      expect(fp1).not.toBe(fp2);
    });
  });

  describe('Signing and Verification', () => {
    it('should sign and verify messages', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const message = 'Hello, Rinku!';
      
      const signature = await sign(message, privateKey);
      const isValid = await verify(message, signature, publicKey);
      
      expect(isValid).toBe(true);
    });

    it('should reject invalid signatures (wrong key)', async () => {
      const kp1 = await generateKeyPair();
      const kp2 = await generateKeyPair();
      const message = 'Hello, Rinku!';
      
      const signature = await sign(message, kp1.privateKey);
      const isValid = await verify(message, signature, kp2.publicKey);
      
      expect(isValid).toBe(false);
    });

    it('should reject tampered messages', async () => {
      const { publicKey, privateKey } = await generateKeyPair();
      const message = 'Hello, Rinku!';
      
      const signature = await sign(message, privateKey);
      const isValid = await verify('Tampered message', signature, publicKey);
      
      expect(isValid).toBe(false);
    });
  });

  describe('Key Serialization', () => {
    it('should serialize and deserialize key pairs', async () => {
      const original = await generateKeyPair();
      const message = 'Test message';
      
      const serialized = serializeKeyPair(original);
      const restored = deserializeKeyPair(serialized);
      
      const sig = await sign(message, restored.privateKey);
      const isValid = await verify(message, sig, restored.publicKey);
      
      expect(isValid).toBe(true);
    });

    it('should preserve fingerprint after serialization', async () => {
      const original = await generateKeyPair();
      
      const serialized = serializeKeyPair(original);
      const restored = deserializeKeyPair(serialized);
      
      const fp1 = await computeFingerprint(original.publicKey);
      const fp2 = await computeFingerprint(restored.publicKey);
      
      expect(fp1).toBe(fp2);
    });
  });
});
