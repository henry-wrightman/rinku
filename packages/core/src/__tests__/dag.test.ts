import { describe, it, expect } from 'vitest';
import { DAG } from '../dag.js';
import { generateKeyPair, computeFingerprint, sign, hashTransaction } from '../crypto.js';
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
      nonce,
      tipUrls,
      sig: 'mocksig123',
      ts: Date.now(),
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
      const tx2 = await createMockTx('charlie', 'dave', 200, 1);
      
      await dag.addTransaction(tx1);
      await dag.addTransaction(tx2);
      
      const tips = dag.getTips();
      expect(tips.length).toBe(2);
      expect(tips).toContain(tx1.hash);
      expect(tips).toContain(tx2.hash);
    });

    it('should merge tips when referenced', async () => {
      const dag = new DAG();
      
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      const tx2 = await createMockTx('charlie', 'dave', 200, 1);
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
        const tx = await createMockTx('alice', 'bob', 100, i);
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
  });

  describe('Serialization', () => {
    it('should serialize and deserialize', async () => {
      const dag = new DAG();
      
      const tx1 = await createMockTx('alice', 'bob', 100, 1);
      const tx2 = await createMockTx('charlie', 'dave', 200, 1);
      await dag.addTransaction(tx1);
      await dag.addTransaction(tx2);
      
      const json = dag.toJSON();
      const restored = DAG.fromJSON(json);
      
      expect(restored.getTips().length).toBe(2);
      expect(restored.getNode(tx1.hash)).toBeDefined();
      expect(restored.getNode(tx2.hash)).toBeDefined();
    });
  });
});
