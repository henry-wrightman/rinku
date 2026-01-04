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

  describe('Transaction Merkle Tree', () => {
    it('should compute transaction merkle root', async () => {
      const { getTransactionMerkleRoot } = await import('../merkle.js');
      const txHashes = ['hash1', 'hash2', 'hash3'];
      const root = await getTransactionMerkleRoot(txHashes);
      expect(root).toBeDefined();
      expect(typeof root).toBe('string');
    });

    it('should produce deterministic root for same hashes', async () => {
      const { getTransactionMerkleRoot } = await import('../merkle.js');
      const txHashes = ['abc', 'def', 'ghi'];
      const root1 = await getTransactionMerkleRoot(txHashes);
      const root2 = await getTransactionMerkleRoot(txHashes);
      expect(root1).toBe(root2);
    });

    it('should produce different root for different hashes', async () => {
      const { getTransactionMerkleRoot } = await import('../merkle.js');
      const root1 = await getTransactionMerkleRoot(['a', 'b']);
      const root2 = await getTransactionMerkleRoot(['c', 'd']);
      expect(root1).not.toBe(root2);
    });

    it('should handle empty transaction list', async () => {
      const { getTransactionMerkleRoot } = await import('../merkle.js');
      const root = await getTransactionMerkleRoot([]);
      expect(root).toBeDefined();
    });

    it('should handle single transaction', async () => {
      const { getTransactionMerkleRoot } = await import('../merkle.js');
      const root = await getTransactionMerkleRoot(['onlyhash']);
      expect(root).toBeDefined();
    });
  });

  describe('Transaction Merkle Proofs', () => {
    it('should generate proof for transaction', async () => {
      const { getTransactionMerkleProof } = await import('../merkle.js');
      const txHashes = ['tx1', 'tx2', 'tx3', 'tx4'];
      const proof = await getTransactionMerkleProof(txHashes, 'tx2');
      expect(proof).not.toBeNull();
      expect(proof!.proof).toBeDefined();
      expect(typeof proof!.index).toBe('number');
    });

    it('should return null for non-existent transaction', async () => {
      const { getTransactionMerkleProof } = await import('../merkle.js');
      const txHashes = ['tx1', 'tx2'];
      const proof = await getTransactionMerkleProof(txHashes, 'nonexistent');
      expect(proof).toBeNull();
    });

    it('should verify transaction merkle proof', async () => {
      const { getTransactionMerkleRoot, getTransactionMerkleProof } = await import('../merkle.js');
      const txHashes = ['aaa', 'bbb', 'ccc', 'ddd'];
      const targetHash = 'bbb';
      const root = await getTransactionMerkleRoot(txHashes);
      const proofResult = await getTransactionMerkleProof(txHashes, targetHash);
      expect(proofResult).not.toBeNull();
      const isValid = await verifyMerkleProof(
        targetHash,
        proofResult!.proof,
        proofResult!.index,
        root
      );
      expect(isValid).toBe(true);
    });

    it('should handle odd number of transactions', async () => {
      const { getTransactionMerkleRoot, getTransactionMerkleProof } = await import('../merkle.js');
      const txHashes = ['tx1', 'tx2', 'tx3', 'tx4'];
      const targetHash = 'tx1';
      const root = await getTransactionMerkleRoot(txHashes);
      const proof = await getTransactionMerkleProof(txHashes, targetHash);
      expect(proof).not.toBeNull();
      const isValid = await verifyMerkleProof(
        targetHash,
        proof!.proof,
        proof!.index,
        root
      );
      expect(isValid).toBe(true);
    });
  });
});
