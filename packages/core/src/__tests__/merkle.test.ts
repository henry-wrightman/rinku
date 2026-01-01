import { describe, it, expect } from 'vitest';
import {
  createMerkleTree,
  getMerkleRoot,
  getMerkleProof,
  verifyMerkleProof,
} from '../merkle.js';
import { hash } from '../crypto.js';
import type { AccountState } from '../types.js';

describe('Merkle Module', () => {
  const createAccount = (fingerprint: string, balance: number, nonce: number): AccountState => ({
    fingerprint,
    balance,
    nonce,
    firstTxTimestamp: Date.now(),
  });

  describe('Merkle Tree Creation', () => {
    it('should return null for empty accounts', async () => {
      const accounts = new Map<string, AccountState>();
      const tree = await createMerkleTree(accounts);
      
      expect(tree).toBeNull();
    });

    it('should create tree for single account', async () => {
      const accounts = new Map<string, AccountState>();
      accounts.set('account1', createAccount('account1', 1000, 1));
      
      const tree = await createMerkleTree(accounts);
      
      expect(tree).not.toBeNull();
      expect(tree!.hash).toBeDefined();
    });

    it('should create tree for multiple accounts', async () => {
      const accounts = new Map<string, AccountState>();
      accounts.set('account1', createAccount('account1', 1000, 1));
      accounts.set('account2', createAccount('account2', 2000, 2));
      accounts.set('account3', createAccount('account3', 3000, 3));
      
      const tree = await createMerkleTree(accounts);
      
      expect(tree).not.toBeNull();
      expect(tree!.hash).toBeDefined();
    });
  });

  describe('Merkle Root', () => {
    it('should produce deterministic root', async () => {
      const accounts = new Map<string, AccountState>();
      accounts.set('a', createAccount('a', 100, 1));
      accounts.set('b', createAccount('b', 200, 2));
      
      const root1 = await getMerkleRoot(accounts);
      const root2 = await getMerkleRoot(accounts);
      
      expect(root1).toBe(root2);
    });

    it('should change when balance changes', async () => {
      const accounts1 = new Map<string, AccountState>();
      accounts1.set('a', createAccount('a', 100, 1));
      
      const accounts2 = new Map<string, AccountState>();
      accounts2.set('a', createAccount('a', 200, 1));
      
      const root1 = await getMerkleRoot(accounts1);
      const root2 = await getMerkleRoot(accounts2);
      
      expect(root1).not.toBe(root2);
    });

    it('should handle empty accounts', async () => {
      const accounts = new Map<string, AccountState>();
      const root = await getMerkleRoot(accounts);
      
      expect(root).toBeDefined();
      expect(typeof root).toBe('string');
    });
  });

  describe('Merkle Proofs', () => {
    it('should generate proof for existing account', async () => {
      const accounts = new Map<string, AccountState>();
      accounts.set('a', createAccount('a', 100, 1));
      accounts.set('b', createAccount('b', 200, 2));
      accounts.set('c', createAccount('c', 300, 3));
      
      const proof = await getMerkleProof(accounts, 'b');
      
      expect(proof).not.toBeNull();
      expect(proof!.proof).toBeDefined();
      expect(typeof proof!.index).toBe('number');
    });

    it('should return null for non-existent account', async () => {
      const accounts = new Map<string, AccountState>();
      accounts.set('a', createAccount('a', 100, 1));
      
      const proof = await getMerkleProof(accounts, 'nonexistent');
      
      expect(proof).toBeNull();
    });

    it('should verify valid proof', async () => {
      const accounts = new Map<string, AccountState>();
      const account = createAccount('testaccount', 100, 1);
      accounts.set('testaccount', account);
      accounts.set('other', createAccount('other', 200, 2));
      
      const root = await getMerkleRoot(accounts);
      const proofResult = await getMerkleProof(accounts, 'testaccount');
      
      expect(proofResult).not.toBeNull();
      
      const leafHash = await hash(`testaccount:${account.balance}:${account.nonce}`);
      const isValid = await verifyMerkleProof(
        leafHash,
        proofResult!.proof,
        proofResult!.index,
        root
      );
      
      expect(isValid).toBe(true);
    });

    it('should reject invalid proof', async () => {
      const accounts = new Map<string, AccountState>();
      accounts.set('a', createAccount('a', 100, 1));
      accounts.set('b', createAccount('b', 200, 2));
      
      const root = await getMerkleRoot(accounts);
      const proofResult = await getMerkleProof(accounts, 'a');
      
      const isValid = await verifyMerkleProof(
        'fake_leaf_hash',
        proofResult!.proof,
        proofResult!.index,
        root
      );
      
      expect(isValid).toBe(false);
    });
  });
});
