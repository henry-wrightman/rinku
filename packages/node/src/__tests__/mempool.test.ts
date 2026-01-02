import { describe, it, expect, beforeEach } from 'vitest';
import { Mempool } from '../mempool.js';
import type { SignedTransaction } from '@rinku/core';

describe('Mempool', () => {
  let mempool: Mempool;

  const createMockTx = (hash: string, from: string, nonce: number): SignedTransaction => ({
    hash,
    from,
    to: 'recipient',
    amount: 100,
    fee: 0.01,
    nonce,
    tipUrls: [],
    sig: 'mocksig',
    ts: Date.now(),
  });

  beforeEach(() => {
    mempool = new Mempool();
  });

  describe('Transaction Management', () => {
    it('should add transactions', () => {
      const tx = createMockTx('hash1', 'alice', 1);
      const added = mempool.add(tx);
      
      expect(added).toBe(true);
      expect(mempool.size()).toBe(1);
      expect(mempool.has('hash1')).toBe(true);
    });

    it('should not add duplicate transactions', () => {
      const tx = createMockTx('hash1', 'alice', 1);
      mempool.add(tx);
      const added = mempool.add(tx);
      
      expect(added).toBe(false);
      expect(mempool.size()).toBe(1);
    });

    it('should get transaction by hash', () => {
      const tx = createMockTx('hash1', 'alice', 1);
      mempool.add(tx);
      
      const retrieved = mempool.get('hash1');
      expect(retrieved).toEqual(tx);
    });

    it('should return undefined for missing transaction', () => {
      expect(mempool.get('nonexistent')).toBeUndefined();
    });

    it('should remove transactions', () => {
      const tx = createMockTx('hash1', 'alice', 1);
      mempool.add(tx);
      mempool.remove('hash1');
      
      expect(mempool.size()).toBe(0);
      expect(mempool.has('hash1')).toBe(false);
    });
  });

  describe('Transaction Selection', () => {
    it('should get all transactions', () => {
      mempool.add(createMockTx('hash1', 'alice', 1));
      mempool.add(createMockTx('hash2', 'bob', 1));
      mempool.add(createMockTx('hash3', 'charlie', 1));
      
      const all = mempool.getAll();
      expect(all.length).toBe(3);
    });

    it('should get transactions by account', () => {
      mempool.add(createMockTx('hash1', 'alice', 1));
      mempool.add(createMockTx('hash2', 'alice', 2));
      mempool.add(createMockTx('hash3', 'bob', 1));
      
      const aliceTxs = mempool.getByAccount('alice');
      expect(aliceTxs.length).toBe(2);
    });
  });

  describe('Capacity', () => {
    it('should respect max size', () => {
      const smallMempool = new Mempool(3);
      
      smallMempool.add(createMockTx('hash1', 'a', 1));
      smallMempool.add(createMockTx('hash2', 'b', 1));
      smallMempool.add(createMockTx('hash3', 'c', 1));
      const added = smallMempool.add(createMockTx('hash4', 'd', 1));
      
      expect(added).toBe(false);
      expect(smallMempool.size()).toBe(3);
    });
  });

  describe('Clear Operations', () => {
    it('should clear all transactions', () => {
      mempool.add(createMockTx('hash1', 'alice', 1));
      mempool.add(createMockTx('hash2', 'bob', 1));
      
      mempool.clear();
      
      expect(mempool.size()).toBe(0);
    });
  });
});
