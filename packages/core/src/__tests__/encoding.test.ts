import { describe, it, expect } from 'vitest';
import {
  encodeTransaction,
  decodeTransaction,
  createTransactionURL,
  parseTransactionURL,
  base64urlEncode,
  base64urlDecode,
} from '../encoding.js';
import type { Transaction } from '../types.js';

describe('Encoding Module', () => {
  const mockTransaction: Transaction = {
    from: 'abc123def456',
    to: 'xyz789uvw012',
    amount: 1000,
    nonce: 1,
    tipUrls: ['/tx/parent1', '/tx/parent2'],
    sig: 'mocksignature123',
    ts: 1700000000000,
  };

  describe('Base64URL', () => {
    it('should encode and decode strings', () => {
      const original = 'Hello, Rinku!';
      const encoded = base64urlEncode(new TextEncoder().encode(original));
      const decoded = new TextDecoder().decode(base64urlDecode(encoded));
      
      expect(decoded).toBe(original);
    });

    it('should handle binary data correctly', () => {
      const data = new Uint8Array([255, 254, 253, 0, 1, 2]);
      const encoded = base64urlEncode(data);
      const decoded = base64urlDecode(encoded);
      
      expect(decoded).toEqual(data);
    });

    it('should not contain URL-unsafe characters', () => {
      const data = new Uint8Array([255, 254, 253, 0, 1, 2]);
      const encoded = base64urlEncode(data);
      
      expect(encoded).not.toContain('+');
      expect(encoded).not.toContain('/');
    });
  });

  describe('Transaction Encoding', () => {
    it('should encode and decode transactions', () => {
      const encoded = encodeTransaction(mockTransaction);
      const decoded = decodeTransaction(encoded);
      
      expect(decoded.from).toBe(mockTransaction.from);
      expect(decoded.to).toBe(mockTransaction.to);
      expect(decoded.amount).toBe(mockTransaction.amount);
      expect(decoded.nonce).toBe(mockTransaction.nonce);
      expect(decoded.tipUrls).toEqual(mockTransaction.tipUrls);
    });

    it('should produce encoded output', () => {
      const encoded = encodeTransaction(mockTransaction);
      
      expect(encoded.length).toBeGreaterThan(0);
      expect(typeof encoded).toBe('string');
    });
  });

  describe('URL Encoding', () => {
    it('should convert transactions to URLs', () => {
      const { path } = createTransactionURL(mockTransaction);
      
      expect(path).toMatch(/^\/tx\/.+$/);
    });

    it('should parse transaction URLs', () => {
      const { path } = createTransactionURL(mockTransaction);
      const parsed = parseTransactionURL(path);
      
      expect(parsed).not.toBeNull();
      expect(parsed!.from).toBe(mockTransaction.from);
      expect(parsed!.to).toBe(mockTransaction.to);
      expect(parsed!.amount).toBe(mockTransaction.amount);
    });

    it('should return null for invalid URL formats', () => {
      expect(parseTransactionURL('/invalid/format')).toBeNull();
      expect(parseTransactionURL('not-a-url')).toBeNull();
    });
  });

  describe('Edge Cases', () => {
    it('should handle zero amounts', () => {
      const tx = { ...mockTransaction, amount: 0 };
      const { path } = createTransactionURL(tx);
      const parsed = parseTransactionURL(path);
      
      expect(parsed!.amount).toBe(0);
    });

    it('should handle large amounts', () => {
      const tx = { ...mockTransaction, amount: 1e15 };
      const { path } = createTransactionURL(tx);
      const parsed = parseTransactionURL(path);
      
      expect(parsed!.amount).toBe(1e15);
    });

    it('should handle empty tipUrls', () => {
      const tx = { ...mockTransaction, tipUrls: [] };
      const { path } = createTransactionURL(tx);
      const parsed = parseTransactionURL(path);
      
      expect(parsed!.tipUrls).toEqual([]);
    });
  });
});
