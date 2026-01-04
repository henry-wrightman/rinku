import { describe, it, expect, beforeEach } from 'vitest';
import { StateManager } from '../state.js';
import type { SignedTransaction } from '@rinku/core';

describe('StateManager', () => {
  let state: StateManager;

  beforeEach(() => {
    state = new StateManager();
  });

  describe('Account Management', () => {
    it('should start with no accounts', () => {
      const accounts = state.getAllAccounts();
      expect(accounts.size).toBe(0);
    });

    it('should create account', () => {
      state.createAccount('alice', 1000);
      
      const account = state.getAccount('alice');
      expect(account).toBeDefined();
      expect(account!.balance).toBe(1000);
      expect(account!.nonce).toBe(0);
    });

    it('should set faucet account', () => {
      state.setFaucetAccount('faucet', 1000000);
      
      const account = state.getAccount('faucet');
      expect(account).toBeDefined();
      expect(account!.balance).toBe(1000000);
    });
  });

  describe('Balance Operations', () => {
    it('should update balance positively', async () => {
      state.createAccount('alice', 100);
      
      const success = await state.updateBalance('alice', 50);
      
      expect(success).toBe(true);
      expect(state.getAccount('alice')!.balance).toBe(150);
    });

    it('should update balance negatively', async () => {
      state.createAccount('alice', 100);
      
      const success = await state.updateBalance('alice', -30);
      
      expect(success).toBe(true);
      expect(state.getAccount('alice')!.balance).toBe(70);
    });

    it('should reject negative final balance', async () => {
      state.createAccount('alice', 100);
      
      const success = await state.updateBalance('alice', -200);
      
      expect(success).toBe(false);
      expect(state.getAccount('alice')!.balance).toBe(100);
    });

    it('should create account on positive update to new address', async () => {
      const success = await state.updateBalance('newuser', 100);
      
      expect(success).toBe(true);
      expect(state.getAccount('newuser')!.balance).toBe(100);
    });
  });

  describe('Transaction Application', () => {
    it('should apply transaction between accounts', async () => {
      state.createAccount('alice', 1000);
      
      const tx: SignedTransaction = {
        hash: 'txhash1',
        from: 'alice',
        to: 'bob',
        amount: 300,
        fee: 0,
        nonce: 1,
        tipUrls: [],
        sig: 'sig',
        ts: Date.now(),
      };
      
      const success = await state.applyTransaction(tx);
      
      expect(success).toBe(true);
      expect(state.getAccount('alice')!.balance).toBe(700);
      expect(state.getAccount('bob')!.balance).toBe(300);
    });

    it('should reject insufficient balance', async () => {
      state.createAccount('alice', 100);
      
      const tx: SignedTransaction = {
        hash: 'txhash1',
        from: 'alice',
        to: 'bob',
        amount: 500,
        fee: 0,
        nonce: 1,
        tipUrls: [],
        sig: 'sig',
        ts: Date.now(),
      };
      
      const success = await state.applyTransaction(tx);
      
      expect(success).toBe(false);
    });

    it('should reject invalid nonce', async () => {
      state.createAccount('alice', 1000);
      
      const tx: SignedTransaction = {
        hash: 'txhash1',
        from: 'alice',
        to: 'bob',
        amount: 100,
        fee: 0,
        nonce: 5,
        tipUrls: [],
        sig: 'sig',
        ts: Date.now(),
      };
      
      const success = await state.applyTransaction(tx);
      
      expect(success).toBe(false);
    });

    it('should allow genesis transactions with skipChecks', async () => {
      const tx: SignedTransaction = {
        hash: 'txhash1',
        from: 'genesis',
        to: 'faucet',
        amount: 1000000,
        fee: 0,
        nonce: 0,
        tipUrls: [],
        sig: 'sig',
        ts: Date.now(),
      };
      
      const success = await state.applyTransaction(tx, { skipChecks: true });
      
      expect(success).toBe(true);
      expect(state.getAccount('faucet')!.balance).toBe(1000000);
    });
  });

  describe('Merkle Root', () => {
    it('should update merkle root', async () => {
      state.createAccount('alice', 1000);
      
      const root = await state.updateMerkleRootIfNeeded();
      
      expect(root).toBeDefined();
      expect(typeof root).toBe('string');
      expect(state.getMerkleRoot()).toBe(root);
    });
  });

  describe('Serialization', () => {
    it('should serialize and restore state', () => {
      state.createAccount('alice', 1000);
      state.createAccount('bob', 500);
      
      const json = state.toJSON();
      const restored = StateManager.fromJSON(json);
      
      expect(restored.getAccount('alice')!.balance).toBe(1000);
      expect(restored.getAccount('bob')!.balance).toBe(500);
    });
  });

  describe('Consolidation Transactions', () => {
    it('should apply zero-fee consolidation transaction', async () => {
      state.createAccount('validator', 0);
      
      const tx: SignedTransaction = {
        hash: 'consolidation1',
        from: 'validator',
        to: 'validator',
        amount: 0,
        fee: 0,
        nonce: 1,
        tipUrls: ['rinku://tx/tip1', 'rinku://tx/tip2'],
        sig: 'sig',
        ts: Date.now(),
        kind: 'consolidation',
      };
      
      const success = await state.applyTransaction(tx);
      
      expect(success).toBe(true);
      expect(state.getAccount('validator')!.nonce).toBe(1);
    });

    it('should increment nonce on consolidation transaction', async () => {
      state.createAccount('validator', 0);
      
      const tx1: SignedTransaction = {
        hash: 'consolidation1',
        from: 'validator',
        to: 'validator',
        amount: 0,
        fee: 0,
        nonce: 1,
        tipUrls: ['rinku://tx/tip1'],
        sig: 'sig',
        ts: Date.now(),
        kind: 'consolidation',
      };
      
      await state.applyTransaction(tx1);
      expect(state.getAccount('validator')!.nonce).toBe(1);
      
      const tx2: SignedTransaction = {
        hash: 'consolidation2',
        from: 'validator',
        to: 'validator',
        amount: 0,
        fee: 0,
        nonce: 2,
        tipUrls: ['rinku://tx/tip2'],
        sig: 'sig',
        ts: Date.now(),
        kind: 'consolidation',
      };
      
      await state.applyTransaction(tx2);
      expect(state.getAccount('validator')!.nonce).toBe(2);
    });

    it('should reject replay of consolidation transaction (same nonce)', async () => {
      state.createAccount('validator', 0);
      
      const tx1: SignedTransaction = {
        hash: 'consolidation1',
        from: 'validator',
        to: 'validator',
        amount: 0,
        fee: 0,
        nonce: 1,
        tipUrls: ['rinku://tx/tip1'],
        sig: 'sig',
        ts: Date.now(),
        kind: 'consolidation',
      };
      
      const success1 = await state.applyTransaction(tx1);
      expect(success1).toBe(true);
      
      const replayTx: SignedTransaction = {
        hash: 'consolidation1-replay',
        from: 'validator',
        to: 'validator',
        amount: 0,
        fee: 0,
        nonce: 1,
        tipUrls: ['rinku://tx/tip1'],
        sig: 'sig',
        ts: Date.now(),
        kind: 'consolidation',
      };
      
      const success2 = await state.applyTransaction(replayTx);
      expect(success2).toBe(false);
      expect(state.getAccount('validator')!.nonce).toBe(1);
    });

    it('should reject consolidation from non-existent account', async () => {
      const tx: SignedTransaction = {
        hash: 'consolidation1',
        from: 'unknown-validator',
        to: 'unknown-validator',
        amount: 0,
        fee: 0,
        nonce: 1,
        tipUrls: ['rinku://tx/tip1'],
        sig: 'sig',
        ts: Date.now(),
        kind: 'consolidation',
      };
      
      const success = await state.applyTransaction(tx);
      expect(success).toBe(false);
    });
  });
});
