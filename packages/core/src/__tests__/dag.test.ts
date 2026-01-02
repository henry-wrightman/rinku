import { describe, it, expect } from 'vitest';
import { DAG } from '../dag.js';
import { hashTransaction } from '../crypto.js';
import { createTransactionURL } from '../encoding.js';
import type { SignedTransaction } from '../types.js';

describe('DAG Module', () => {
  const createMockTx = async (
    from: string,
    to: string,
    amount: number,
    nonce: number,
    tipUrls: string[] = []
  ): Promise<SignedTransaction> => {
    const tx = {
      from,
      to,
      amount,
      fee: 0,
      nonce,
      tipUrls,
      sig: 'mocksig123',
      ts: Date.now() + nonce,
    };
    const txHash = await hashTransaction(tx);
    return { ...tx, hash: txHash };
  };

  describe('Basic Operations', () => {
    it('should start with empty tips', () => {
      const dag = new DAG();
      expect(dag.getTips()).toEqual([]);
    });

    it('should add transactions', async () => {
      const dag = new DAG();
      const tx = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx);
      expect(dag.getNode(tx.hash)).toBeDefined();
    });

    it('should track tips correctly', async () => {
      const dag = new DAG();
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx1);
      expect(dag.getTips()).toContain(tx1.hash);
    });

    it('should get all nodes', async () => {
      const dag = new DAG();
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      const tx2 = await createMockTx('charlie', 'dave', 200, 2);
      await dag.addTransaction(tx1);
      await dag.addTransaction(tx2);
      expect(dag.getAllNodes().length).toBe(2);
    });

    it('should return undefined for non-existent node', () => {
      const dag = new DAG();
      expect(dag.getNode('nonexistent')).toBeUndefined();
    });
  });

  describe('Parent-Child Relationships', () => {
    it('should remove parent from tips when referenced', async () => {
      const dag = new DAG();
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx1);
      const parentUrl = createTransactionURL(tx1).path;
      const tx2 = await createMockTx('bob', 'charlie', 50, 1, [parentUrl]);
      await dag.addTransaction(tx2);
      expect(dag.getTips()).not.toContain(tx1.hash);
      expect(dag.getTips()).toContain(tx2.hash);
    });

    it('should track children correctly', async () => {
      const dag = new DAG();
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx1);
      const parentUrl = createTransactionURL(tx1).path;
      const tx2 = await createMockTx('bob', 'charlie', 50, 1, [parentUrl]);
      await dag.addTransaction(tx2);
      const node1 = dag.getNode(tx1.hash);
      expect(node1?.children).toContain(tx2.hash);
    });
  });

  describe('Multiple Tips (DAG Property)', () => {
    it('should support multiple tips', async () => {
      const dag = new DAG();
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      const tx2 = await createMockTx('charlie', 'dave', 200, 2);
      await dag.addTransaction(tx1);
      await dag.addTransaction(tx2);
      const tips = dag.getTips();
      expect(tips.length).toBe(2);
    });

    it('should merge tips when referenced', async () => {
      const dag = new DAG();
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      const tx2 = await createMockTx('charlie', 'dave', 200, 2);
      await dag.addTransaction(tx1);
      await dag.addTransaction(tx2);
      const url1 = createTransactionURL(tx1).path;
      const url2 = createTransactionURL(tx2).path;
      const tx3 = await createMockTx('eve', 'frank', 50, 1, [url1, url2]);
      await dag.addTransaction(tx3);
      const tips = dag.getTips();
      expect(tips.length).toBe(1);
      expect(tips[0]).toBe(tx3.hash);
    });
  });

  describe('Tip Selection', () => {
    it('should select requested number of tips', async () => {
      const dag = new DAG();
      for (let i = 0; i < 5; i++) {
        const tx = await createMockTx(`user${i}`, 'bob', 100, i);
        await dag.addTransaction(tx);
      }
      const selected = dag.selectTips(2);
      expect(selected.length).toBe(2);
    });

    it('should return all tips if fewer than requested', async () => {
      const dag = new DAG();
      const tx = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx);
      const selected = dag.selectTips(5);
      expect(selected.length).toBe(1);
    });

    it('should return empty for empty DAG', () => {
      const dag = new DAG();
      const selected = dag.selectTips(2);
      expect(selected).toEqual([]);
    });

    it('should select tip URLs', async () => {
      const dag = new DAG();
      const tx = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx);
      const urls = dag.selectTipUrls(1);
      expect(urls.length).toBe(1);
      expect(urls[0]).toMatch(/^\/tx\//);
    });

    it('should get tip URLs', async () => {
      const dag = new DAG();
      const tx = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx);
      const urls = dag.getTipUrls();
      expect(urls.length).toBe(1);
      expect(urls[0]).toMatch(/^\/tx\//);
    });
  });

  describe('Weight Management', () => {
    it('should update weights from account weights', async () => {
      const dag = new DAG();
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx1);
      const accountWeights = new Map([['alice', 50]]);
      dag.updateWeights(accountWeights);
      const node = dag.getNode(tx1.hash);
      expect(node?.weight).toBe(50);
    });

    it('should propagate weights to children', async () => {
      const dag = new DAG();
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx1);
      const url1 = createTransactionURL(tx1).path;
      const tx2 = await createMockTx('bob', 'charlie', 50, 1, [url1]);
      await dag.addTransaction(tx2);
      const accountWeights = new Map([['alice', 50], ['bob', 30]]);
      dag.updateWeights(accountWeights);
      const childNode = dag.getNode(tx2.hash);
      expect(childNode?.weight).toBeGreaterThanOrEqual(30);
    });
  });

  describe('Conflict Resolution', () => {
    it('should resolve conflict in favor of higher weight', async () => {
      const dag = new DAG();
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      const tx2 = await createMockTx('charlie', 'dave', 200, 2);
      await dag.addTransaction(tx1);
      await dag.addTransaction(tx2);
      const accountWeights = new Map([['alice', 100], ['charlie', 50]]);
      dag.updateWeights(accountWeights);
      const winner = dag.resolveConflict(tx1.hash, tx2.hash);
      expect(winner).toBe(tx1.hash);
    });

    it('should throw for non-existent transaction', async () => {
      const dag = new DAG();
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx1);
      expect(() => dag.resolveConflict(tx1.hash, 'nonexistent')).toThrow('Transaction not found');
    });
  });

  describe('Ancestry', () => {
    it('should find ancestors', async () => {
      const dag = new DAG();
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx1);
      const url1 = createTransactionURL(tx1).path;
      const tx2 = await createMockTx('bob', 'charlie', 50, 1, [url1]);
      await dag.addTransaction(tx2);
      const ancestors = dag.getAncestors(tx2.hash);
      expect(ancestors.has(tx1.hash)).toBe(true);
    });

    it('should find descendants', async () => {
      const dag = new DAG();
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx1);
      const url1 = createTransactionURL(tx1).path;
      const tx2 = await createMockTx('bob', 'charlie', 50, 1, [url1]);
      await dag.addTransaction(tx2);
      const descendants = dag.getDescendants(tx1.hash);
      expect(descendants.has(tx2.hash)).toBe(true);
    });

    it('should return empty for no ancestors', async () => {
      const dag = new DAG();
      const tx = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx);
      const ancestors = dag.getAncestors(tx.hash);
      expect(ancestors.size).toBe(0);
    });

    it('should return empty for no descendants', async () => {
      const dag = new DAG();
      const tx = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx);
      const descendants = dag.getDescendants(tx.hash);
      expect(descendants.size).toBe(0);
    });
  });

  describe('URL Resolution', () => {
    it('should resolve URL to hash', async () => {
      const dag = new DAG();
      const tx = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx);
      const url = createTransactionURL(tx).path;
      const hash = dag.resolveUrlToHash(url);
      expect(hash).toBe(tx.hash);
    });

    it('should return null for unknown URL', () => {
      const dag = new DAG();
      const hash = dag.resolveUrlToHash('/tx/unknown');
      expect(hash).toBeNull();
    });
  });

  describe('Serialization', () => {
    it('should serialize and deserialize', async () => {
      const dag = new DAG();
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      const tx2 = await createMockTx('charlie', 'dave', 200, 2);
      await dag.addTransaction(tx1);
      await dag.addTransaction(tx2);
      const json = dag.toJSON();
      const restored = DAG.fromJSON(json);
      expect(restored.getTips().length).toBe(2);
      expect(restored.getNode(tx1.hash)).toBeDefined();
      expect(restored.getNode(tx2.hash)).toBeDefined();
    });

    it('should handle legacy format with parents field', () => {
      const legacyData = {
        nodes: [{
          hash: 'abc123',
          tx: { from: 'alice', to: 'bob', amount: 100, fee: 0, nonce: 1, tipUrls: [], sig: 'sig', ts: 123, hash: 'abc123' },
          parents: ['/tx/parent1'],
          children: [],
          weight: 0,
          confirmed: false
        }],
        tipHashes: ['abc123']
      };
      const dag = DAG.fromJSON(legacyData);
      expect(dag.getNode('abc123')).toBeDefined();
    });

    it('should rebuild urlToHash on deserialization', async () => {
      const dag = new DAG();
      const tx = await createMockTx('alice', 'bob', 100, 1);
      await dag.addTransaction(tx);
      const json = dag.toJSON() as any;
      expect(json.urlToHash).toBeUndefined();
      const restored = DAG.fromJSON(json);
      const url = createTransactionURL(tx).path;
      expect(restored.resolveUrlToHash(url)).toBe(tx.hash);
    });
  });
});
