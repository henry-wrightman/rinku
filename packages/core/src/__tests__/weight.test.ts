import { describe, it, expect } from 'vitest';
import { calculateWeight, calculateAccountWeights, normalizeWeights } from '../weight.js';
import type { AccountState } from '../types.js';

describe('Weight Module', () => {
  const createAccount = (
    balance: number,
    ageDays: number = 0
  ): AccountState => {
    const now = Date.now();
    const ageMs = ageDays * 24 * 60 * 60 * 1000;
    return {
      fingerprint: 'test',
      balance,
      nonce: 1,
      firstTxTimestamp: now - ageMs,
    };
  };

  describe('Weight Calculation', () => {
    it('should calculate weight from age and balance', () => {
      const account = createAccount(1000, 30);
      const weight = calculateWeight(account);
      
      expect(weight.total).toBeGreaterThan(0);
      expect(weight.accountAge).toBeGreaterThan(0);
      expect(weight.balance).toBeGreaterThan(0);
    });

    it('should increase with age', () => {
      const young = createAccount(1000, 10);
      const old = createAccount(1000, 100);
      
      const youngWeight = calculateWeight(young);
      const oldWeight = calculateWeight(old);
      
      expect(oldWeight.total).toBeGreaterThan(youngWeight.total);
    });

    it('should increase with balance', () => {
      const poor = createAccount(100, 30);
      const rich = createAccount(10000, 30);
      
      const poorWeight = calculateWeight(poor);
      const richWeight = calculateWeight(rich);
      
      expect(richWeight.total).toBeGreaterThan(poorWeight.total);
    });

    it('should handle zero balance (age gated behind stake)', () => {
      const account = createAccount(0, 30);
      const weight = calculateWeight(account);
      
      expect(weight.balance).toBe(0);
      expect(weight.accountAge).toBe(0);
      expect(weight.total).toBe(0);
    });

    it('should handle new account (zero age)', () => {
      const account = createAccount(1000, 0);
      const weight = calculateWeight(account);
      
      expect(weight.accountAge).toBeCloseTo(0, 1);
      expect(weight.balance).toBeGreaterThan(0);
    });
  });

  describe('Batch Weight Calculation', () => {
    it('should calculate weights for multiple accounts', () => {
      const accounts = new Map<string, AccountState>();
      accounts.set('alice', createAccount(1000, 30));
      accounts.set('bob', createAccount(500, 60));
      
      const weights = calculateAccountWeights(accounts);
      
      expect(weights.size).toBe(2);
      expect(weights.get('alice')).toBeGreaterThan(0);
      expect(weights.get('bob')).toBeGreaterThan(0);
    });
  });

  describe('Weight Normalization', () => {
    it('should normalize weights to sum to 1', () => {
      const weights = new Map<string, number>();
      weights.set('a', 100);
      weights.set('b', 200);
      weights.set('c', 300);
      
      const normalized = normalizeWeights(weights);
      
      const sum = Array.from(normalized.values()).reduce((a, b) => a + b, 0);
      expect(sum).toBeCloseTo(1, 5);
    });

    it('should preserve relative weights', () => {
      const weights = new Map<string, number>();
      weights.set('a', 100);
      weights.set('b', 200);
      
      const normalized = normalizeWeights(weights);
      
      expect(normalized.get('b')! / normalized.get('a')!).toBeCloseTo(2, 5);
    });

    it('should handle zero total weight', () => {
      const weights = new Map<string, number>();
      weights.set('a', 0);
      weights.set('b', 0);
      
      const normalized = normalizeWeights(weights);
      
      expect(normalized.get('a')).toBe(0);
      expect(normalized.get('b')).toBe(0);
    });
  });

  describe('Sybil Resistance Properties', () => {
    it('should not favor splitting accounts', () => {
      const singleAccount = createAccount(10000, 30);
      const singleWeight = calculateWeight(singleAccount);
      
      let splitWeight = 0;
      for (let i = 0; i < 10; i++) {
        const splitAccount = createAccount(1000, 30);
        splitWeight += calculateWeight(splitAccount).total;
      }
      
      expect(singleWeight.total).toBeGreaterThanOrEqual(splitWeight * 0.9);
    });

    it('should require time to build significant weight', () => {
      const newRich = createAccount(1000000, 0);
      const oldModest = createAccount(1000, 365);
      
      const newWeight = calculateWeight(newRich);
      const oldWeight = calculateWeight(oldModest);
      
      expect(oldWeight.accountAge).toBeGreaterThan(newWeight.accountAge);
    });
  });
});
