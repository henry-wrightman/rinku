import { describe, it, expect, beforeEach } from 'vitest';
import { StateTrie, ReceiptsTrie } from '../state-trie.js';

describe('StateTrie', () => {
  let trie: StateTrie;

  beforeEach(() => {
    trie = new StateTrie();
  });

  describe('basic operations', () => {
    it('should set and get values', async () => {
      await trie.set('contract1', 'balance', 100);
      const value = await trie.get('contract1', 'balance');
      expect(value).toBe(100);
    });

    it('should return undefined for non-existent keys', async () => {
      const value = await trie.get('contract1', 'nonexistent');
      expect(value).toBeUndefined();
    });

    it('should delete values', async () => {
      await trie.set('contract1', 'balance', 100);
      await trie.delete('contract1', 'balance');
      const value = await trie.get('contract1', 'balance');
      expect(value).toBeUndefined();
    });

    it('should handle multiple contracts', async () => {
      await trie.set('contract1', 'balance', 100);
      await trie.set('contract2', 'balance', 200);
      expect(await trie.get('contract1', 'balance')).toBe(100);
      expect(await trie.get('contract2', 'balance')).toBe(200);
    });

    it('should handle complex values', async () => {
      const complexValue = { users: ['alice', 'bob'], count: 2 };
      await trie.set('contract1', 'data', complexValue);
      const value = await trie.get('contract1', 'data');
      expect(value).toEqual(complexValue);
    });
  });

  describe('getContractState', () => {
    it('should return all keys for a contract', async () => {
      await trie.set('contract1', 'balance', 100);
      await trie.set('contract1', 'owner', 'alice');
      await trie.set('contract2', 'balance', 200);

      const state = await trie.getContractState('contract1');
      expect(state).toEqual({ balance: 100, owner: 'alice' });
    });

    it('should return empty object for non-existent contract', async () => {
      const state = await trie.getContractState('nonexistent');
      expect(state).toEqual({});
    });
  });

  describe('setContractState', () => {
    it('should set entire contract state', async () => {
      await trie.setContractState('contract1', { balance: 100, owner: 'alice' });
      expect(await trie.get('contract1', 'balance')).toBe(100);
      expect(await trie.get('contract1', 'owner')).toBe('alice');
    });

    it('should replace existing state', async () => {
      await trie.set('contract1', 'oldKey', 'oldValue');
      await trie.setContractState('contract1', { newKey: 'newValue' });
      expect(await trie.get('contract1', 'oldKey')).toBeUndefined();
      expect(await trie.get('contract1', 'newKey')).toBe('newValue');
    });
  });

  describe('Merkle root', () => {
    it('should compute root for empty trie', async () => {
      const root = await trie.getRoot();
      expect(root).toBeDefined();
      expect(typeof root).toBe('string');
      expect(root.length).toBeGreaterThan(0);
    });

    it('should compute deterministic root', async () => {
      await trie.set('contract1', 'balance', 100);
      const root1 = await trie.getRoot();
      
      const trie2 = new StateTrie();
      await trie2.set('contract1', 'balance', 100);
      const root2 = await trie2.getRoot();
      
      expect(root1).toBe(root2);
    });

    it('should change root when data changes', async () => {
      await trie.set('contract1', 'balance', 100);
      const root1 = await trie.getRoot();
      
      await trie.set('contract1', 'balance', 200);
      const root2 = await trie.getRoot();
      
      expect(root1).not.toBe(root2);
    });

    it('should handle single leaf correctly', async () => {
      await trie.set('contract1', 'key', 'value');
      const root = await trie.getRoot();
      expect(root).toBeDefined();
      expect(root.length).toBe(64);
    });
  });

  describe('Merkle proofs', () => {
    it('should generate proof for existing key', async () => {
      await trie.set('contract1', 'balance', 100);
      await trie.set('contract1', 'owner', 'alice');
      
      const proof = await trie.getProof('contract1', 'balance');
      expect(proof).not.toBeNull();
      expect(proof!.key).toBe('contract1:balance');
      expect(proof!.value).toBe(100);
      expect(proof!.proof).toBeDefined();
      expect(proof!.index).toBeGreaterThanOrEqual(0);
    });

    it('should return null for non-existent key', async () => {
      const proof = await trie.getProof('contract1', 'nonexistent');
      expect(proof).toBeNull();
    });

    it('should verify valid proof', async () => {
      await trie.set('contract1', 'balance', 100);
      await trie.set('contract1', 'owner', 'alice');
      await trie.set('contract2', 'data', 'test');
      
      const root = await trie.getRoot();
      const proof = await trie.getProof('contract1', 'balance');
      
      expect(proof).not.toBeNull();
      const valid = await trie.verifyProof(
        proof!.key,
        proof!.value,
        proof!.proof,
        proof!.index,
        root
      );
      expect(valid).toBe(true);
    });

    it('should reject invalid proof with wrong value', async () => {
      await trie.set('contract1', 'balance', 100);
      await trie.set('contract1', 'owner', 'alice');
      
      const root = await trie.getRoot();
      const proof = await trie.getProof('contract1', 'balance');
      
      const valid = await trie.verifyProof(
        proof!.key,
        999,
        proof!.proof,
        proof!.index,
        root
      );
      expect(valid).toBe(false);
    });

    it('should reject invalid proof with wrong root', async () => {
      await trie.set('contract1', 'balance', 100);
      
      const proof = await trie.getProof('contract1', 'balance');
      
      const valid = await trie.verifyProof(
        proof!.key,
        proof!.value,
        proof!.proof,
        proof!.index,
        'wrongroot'
      );
      expect(valid).toBe(false);
    });

    it('should handle proof for single-item trie', async () => {
      await trie.set('contract1', 'only', 'value');
      
      const root = await trie.getRoot();
      const proof = await trie.getProof('contract1', 'only');
      
      expect(proof).not.toBeNull();
      expect(proof!.proof.length).toBe(0);
      
      const valid = await trie.verifyProof(
        proof!.key,
        proof!.value,
        proof!.proof,
        proof!.index,
        root
      );
      expect(valid).toBe(true);
    });

    it('should handle proof at odd index', async () => {
      await trie.set('contract1', 'a', 1);
      await trie.set('contract1', 'b', 2);
      await trie.set('contract1', 'c', 3);
      
      const root = await trie.getRoot();
      const proof = await trie.getProof('contract1', 'b');
      
      expect(proof).not.toBeNull();
      const valid = await trie.verifyProof(
        proof!.key,
        proof!.value,
        proof!.proof,
        proof!.index,
        root
      );
      expect(valid).toBe(true);
    });
  });

  describe('computeEffectsHash', () => {
    it('should compute hash for state changes', async () => {
      const changes = [
        { key: 'balance', preValue: 100, postValue: 200 },
        { key: 'owner', preValue: 'alice', postValue: 'bob' }
      ];
      
      const hash = await trie.computeEffectsHash(changes);
      expect(hash).toBeDefined();
      expect(typeof hash).toBe('string');
      expect(hash.length).toBe(64);
    });

    it('should be deterministic', async () => {
      const changes = [
        { key: 'balance', preValue: 100, postValue: 200 }
      ];
      
      const hash1 = await trie.computeEffectsHash(changes);
      const hash2 = await trie.computeEffectsHash(changes);
      expect(hash1).toBe(hash2);
    });

    it('should sort keys for consistency', async () => {
      const changes1 = [
        { key: 'b', preValue: 1, postValue: 2 },
        { key: 'a', preValue: 3, postValue: 4 }
      ];
      const changes2 = [
        { key: 'a', preValue: 3, postValue: 4 },
        { key: 'b', preValue: 1, postValue: 2 }
      ];
      
      const hash1 = await trie.computeEffectsHash(changes1);
      const hash2 = await trie.computeEffectsHash(changes2);
      expect(hash1).toBe(hash2);
    });
  });

  describe('serialization', () => {
    it('should serialize to JSON', async () => {
      await trie.set('contract1', 'balance', 100);
      await trie.set('contract1', 'owner', 'alice');
      
      const json = trie.toJSON();
      expect(json.storage).toBeDefined();
      expect(json.storage.length).toBe(2);
    });

    it('should deserialize from JSON', async () => {
      await trie.set('contract1', 'balance', 100);
      const json = trie.toJSON();
      
      const restored = await StateTrie.fromJSON(json);
      expect(await restored.get('contract1', 'balance')).toBe(100);
    });

    it('should preserve root after deserialization', async () => {
      await trie.set('contract1', 'balance', 100);
      const originalRoot = await trie.getRoot();
      
      const json = trie.toJSON();
      const restored = await StateTrie.fromJSON(json);
      const restoredRoot = await restored.getRoot();
      
      expect(restoredRoot).toBe(originalRoot);
    });
  });

  describe('clone', () => {
    it('should create independent copy', async () => {
      await trie.set('contract1', 'balance', 100);
      const cloned = await trie.clone();
      
      await trie.set('contract1', 'balance', 200);
      
      expect(await trie.get('contract1', 'balance')).toBe(200);
      expect(await cloned.get('contract1', 'balance')).toBe(100);
    });

    it('should preserve root', async () => {
      await trie.set('contract1', 'balance', 100);
      const originalRoot = await trie.getRoot();
      
      const cloned = await trie.clone();
      const clonedRoot = await cloned.getRoot();
      
      expect(clonedRoot).toBe(originalRoot);
    });
  });
});

describe('ReceiptsTrie', () => {
  let trie: ReceiptsTrie;

  beforeEach(() => {
    trie = new ReceiptsTrie();
  });

  describe('basic operations', () => {
    it('should add and get receipts', async () => {
      const receipt = { success: true, result: 42 };
      await trie.addReceipt('call1', receipt);
      
      const retrieved = await trie.getReceipt('call1');
      expect(retrieved).toEqual(receipt);
    });

    it('should return undefined for non-existent receipt', async () => {
      const receipt = await trie.getReceipt('nonexistent');
      expect(receipt).toBeUndefined();
    });

    it('should handle multiple receipts', async () => {
      await trie.addReceipt('call1', { result: 1 });
      await trie.addReceipt('call2', { result: 2 });
      
      expect(await trie.getReceipt('call1')).toEqual({ result: 1 });
      expect(await trie.getReceipt('call2')).toEqual({ result: 2 });
    });
  });

  describe('Merkle root', () => {
    it('should compute root for empty trie', async () => {
      const root = await trie.getRoot();
      expect(root).toBeDefined();
      expect(root.length).toBeGreaterThan(0);
    });

    it('should update root when receipt is added', async () => {
      const root1 = await trie.getRoot();
      await trie.addReceipt('call1', { result: 1 });
      const root2 = await trie.getRoot();
      
      expect(root1).not.toBe(root2);
    });

    it('should be deterministic', async () => {
      await trie.addReceipt('call1', { result: 1 });
      const root1 = await trie.getRoot();
      
      const trie2 = new ReceiptsTrie();
      await trie2.addReceipt('call1', { result: 1 });
      const root2 = await trie2.getRoot();
      
      expect(root1).toBe(root2);
    });
  });

  describe('Merkle proofs', () => {
    it('should generate proof for existing receipt', async () => {
      await trie.addReceipt('call1', { result: 1 });
      await trie.addReceipt('call2', { result: 2 });
      
      const proof = await trie.getProof('call1');
      expect(proof).not.toBeNull();
      expect(proof!.key).toBe('call1');
      expect(proof!.value).toEqual({ result: 1 });
    });

    it('should return null for non-existent receipt', async () => {
      const proof = await trie.getProof('nonexistent');
      expect(proof).toBeNull();
    });

    it('should generate valid proof structure', async () => {
      await trie.addReceipt('call1', { result: 1 });
      await trie.addReceipt('call2', { result: 2 });
      await trie.addReceipt('call3', { result: 3 });
      
      const proof = await trie.getProof('call2');
      expect(proof).not.toBeNull();
      expect(proof!.proof).toBeInstanceOf(Array);
      expect(proof!.index).toBeGreaterThanOrEqual(0);
    });
  });

  describe('clear', () => {
    it('should remove all receipts', async () => {
      await trie.addReceipt('call1', { result: 1 });
      await trie.addReceipt('call2', { result: 2 });
      
      trie.clear();
      
      expect(await trie.getReceipt('call1')).toBeUndefined();
      expect(await trie.getReceipt('call2')).toBeUndefined();
    });

    it('should reset root', async () => {
      await trie.addReceipt('call1', { result: 1 });
      trie.clear();
      
      const root = await trie.getRoot();
      const emptyTrie = new ReceiptsTrie();
      const emptyRoot = await emptyTrie.getRoot();
      
      expect(root).toBe(emptyRoot);
    });
  });

  describe('serialization', () => {
    it('should serialize to JSON', async () => {
      await trie.addReceipt('call1', { result: 1 });
      await trie.addReceipt('call2', { result: 2 });
      
      const json = trie.toJSON();
      expect(json.receipts).toBeDefined();
      expect(json.receipts.length).toBe(2);
    });

    it('should deserialize from JSON', async () => {
      await trie.addReceipt('call1', { result: 1 });
      const json = trie.toJSON();
      
      const restored = await ReceiptsTrie.fromJSON(json);
      expect(await restored.getReceipt('call1')).toEqual({ result: 1 });
    });

    it('should preserve root after deserialization', async () => {
      await trie.addReceipt('call1', { result: 1 });
      const originalRoot = await trie.getRoot();
      
      const json = trie.toJSON();
      const restored = await ReceiptsTrie.fromJSON(json);
      const restoredRoot = await restored.getRoot();
      
      expect(restoredRoot).toBe(originalRoot);
    });
  });
});
