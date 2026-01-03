import { hash as cryptoHash } from './crypto.js';

export interface TrieNode {
  hash: string;
  key?: string;
  value?: unknown;
  children: Map<string, TrieNode>;
}

export interface MerkleProofPath {
  key: string;
  value: unknown;
  proof: string[];
  index: number;
}

export class StateTrie {
  private root: TrieNode;
  private storage: Map<string, unknown> = new Map();

  constructor() {
    this.root = { hash: '', children: new Map() };
  }

  async set(contractId: string, key: string, value: unknown): Promise<void> {
    const fullKey = `${contractId}:${key}`;
    this.storage.set(fullKey, value);
    await this.updateRoot();
  }

  async get(contractId: string, key: string): Promise<unknown> {
    const fullKey = `${contractId}:${key}`;
    return this.storage.get(fullKey);
  }

  async delete(contractId: string, key: string): Promise<void> {
    const fullKey = `${contractId}:${key}`;
    this.storage.delete(fullKey);
    await this.updateRoot();
  }

  async getContractState(contractId: string): Promise<Record<string, unknown>> {
    const prefix = `${contractId}:`;
    const state: Record<string, unknown> = {};
    
    for (const [key, value] of this.storage) {
      if (key.startsWith(prefix)) {
        const localKey = key.slice(prefix.length);
        state[localKey] = value;
      }
    }
    
    return state;
  }

  async setContractState(contractId: string, state: Record<string, unknown>): Promise<void> {
    const prefix = `${contractId}:`;
    for (const key of this.storage.keys()) {
      if (key.startsWith(prefix)) {
        this.storage.delete(key);
      }
    }
    
    for (const [key, value] of Object.entries(state)) {
      this.storage.set(`${prefix}${key}`, value);
    }
    
    await this.updateRoot();
  }

  private async updateRoot(): Promise<void> {
    const sortedKeys = Array.from(this.storage.keys()).sort();
    const leaves: string[] = [];
    
    for (const key of sortedKeys) {
      const value = this.storage.get(key);
      const leafData = JSON.stringify({ key, value });
      const leafHash = await cryptoHash(leafData);
      leaves.push(leafHash);
    }
    
    if (leaves.length === 0) {
      this.root.hash = await cryptoHash('empty');
      return;
    }
    
    this.root.hash = await this.computeMerkleRoot(leaves);
  }

  private async computeMerkleRoot(leaves: string[]): Promise<string> {
    if (leaves.length === 0) return cryptoHash('empty');
    if (leaves.length === 1) return leaves[0];
    
    const nextLevel: string[] = [];
    for (let i = 0; i < leaves.length; i += 2) {
      const left = leaves[i];
      const right = leaves[i + 1] || left;
      const combined = await cryptoHash(left + right);
      nextLevel.push(combined);
    }
    
    return this.computeMerkleRoot(nextLevel);
  }

  async getRoot(): Promise<string> {
    return this.root.hash || await cryptoHash('empty');
  }

  async getProof(contractId: string, key: string): Promise<MerkleProofPath | null> {
    const fullKey = `${contractId}:${key}`;
    const value = this.storage.get(fullKey);
    
    if (value === undefined) return null;
    
    const sortedKeys = Array.from(this.storage.keys()).sort();
    const index = sortedKeys.indexOf(fullKey);
    
    if (index === -1) return null;
    
    const leaves: string[] = [];
    for (const k of sortedKeys) {
      const v = this.storage.get(k);
      const leafData = JSON.stringify({ key: k, value: v });
      const leafHash = await cryptoHash(leafData);
      leaves.push(leafHash);
    }
    
    const proof = await this.generateProof(leaves, index);
    
    return {
      key: fullKey,
      value,
      proof,
      index
    };
  }

  private async generateProof(leaves: string[], targetIndex: number): Promise<string[]> {
    const proof: string[] = [];
    let currentLevel = leaves;
    let idx = targetIndex;
    
    while (currentLevel.length > 1) {
      const siblingIdx = idx % 2 === 0 ? idx + 1 : idx - 1;
      
      if (siblingIdx < currentLevel.length) {
        proof.push(currentLevel[siblingIdx]);
      } else {
        proof.push(currentLevel[idx]);
      }
      
      const nextLevel: string[] = [];
      for (let i = 0; i < currentLevel.length; i += 2) {
        const left = currentLevel[i];
        const right = currentLevel[i + 1] || left;
        const combined = await cryptoHash(left + right);
        nextLevel.push(combined);
      }
      
      currentLevel = nextLevel;
      idx = Math.floor(idx / 2);
    }
    
    return proof;
  }

  async verifyProof(
    key: string,
    value: unknown,
    proof: string[],
    index: number,
    expectedRoot: string
  ): Promise<boolean> {
    const leafData = JSON.stringify({ key, value });
    let currentHash = await cryptoHash(leafData);
    let idx = index;
    
    for (const sibling of proof) {
      if (idx % 2 === 0) {
        currentHash = await cryptoHash(currentHash + sibling);
      } else {
        currentHash = await cryptoHash(sibling + currentHash);
      }
      idx = Math.floor(idx / 2);
    }
    
    return currentHash === expectedRoot;
  }

  async computeEffectsHash(changes: { key: string; preValue: unknown; postValue: unknown }[]): Promise<string> {
    const sorted = changes.sort((a, b) => a.key.localeCompare(b.key));
    const data = JSON.stringify(sorted);
    return cryptoHash(data);
  }

  toJSON(): { storage: [string, unknown][] } {
    return {
      storage: Array.from(this.storage.entries())
    };
  }

  static async fromJSON(data: { storage: [string, unknown][] }): Promise<StateTrie> {
    const trie = new StateTrie();
    for (const [key, value] of data.storage) {
      trie.storage.set(key, value);
    }
    await trie.updateRoot();
    return trie;
  }

  async clone(): Promise<StateTrie> {
    const newTrie = new StateTrie();
    for (const [key, value] of this.storage) {
      newTrie.storage.set(key, JSON.parse(JSON.stringify(value)));
    }
    await newTrie.updateRoot();
    return newTrie;
  }
}

export class ReceiptsTrie {
  private receipts: Map<string, unknown> = new Map();
  private root: string = '';

  async addReceipt(callId: string, receipt: unknown): Promise<void> {
    this.receipts.set(callId, receipt);
    await this.updateRoot();
  }

  async getReceipt(callId: string): Promise<unknown> {
    return this.receipts.get(callId);
  }

  private async updateRoot(): Promise<void> {
    const sortedIds = Array.from(this.receipts.keys()).sort();
    const leaves: string[] = [];
    
    for (const id of sortedIds) {
      const receipt = this.receipts.get(id);
      const leafData = JSON.stringify({ id, receipt });
      const leafHash = await cryptoHash(leafData);
      leaves.push(leafHash);
    }
    
    if (leaves.length === 0) {
      this.root = await cryptoHash('empty-receipts');
      return;
    }
    
    this.root = await this.computeMerkleRoot(leaves);
  }

  private async computeMerkleRoot(leaves: string[]): Promise<string> {
    if (leaves.length === 0) return cryptoHash('empty-receipts');
    if (leaves.length === 1) return leaves[0];
    
    const nextLevel: string[] = [];
    for (let i = 0; i < leaves.length; i += 2) {
      const left = leaves[i];
      const right = leaves[i + 1] || left;
      const combined = await cryptoHash(left + right);
      nextLevel.push(combined);
    }
    
    return this.computeMerkleRoot(nextLevel);
  }

  async getRoot(): Promise<string> {
    if (!this.root) {
      this.root = await cryptoHash('empty-receipts');
    }
    return this.root;
  }

  async getProof(callId: string): Promise<MerkleProofPath | null> {
    const receipt = this.receipts.get(callId);
    if (!receipt) return null;
    
    const sortedIds = Array.from(this.receipts.keys()).sort();
    const index = sortedIds.indexOf(callId);
    
    if (index === -1) return null;
    
    const leaves: string[] = [];
    for (const id of sortedIds) {
      const r = this.receipts.get(id);
      const leafData = JSON.stringify({ id, receipt: r });
      const leafHash = await cryptoHash(leafData);
      leaves.push(leafHash);
    }
    
    const proof = await this.generateProof(leaves, index);
    
    return {
      key: callId,
      value: receipt,
      proof,
      index
    };
  }

  private async generateProof(leaves: string[], targetIndex: number): Promise<string[]> {
    const proof: string[] = [];
    let currentLevel = leaves;
    let idx = targetIndex;
    
    while (currentLevel.length > 1) {
      const siblingIdx = idx % 2 === 0 ? idx + 1 : idx - 1;
      
      if (siblingIdx < currentLevel.length) {
        proof.push(currentLevel[siblingIdx]);
      } else {
        proof.push(currentLevel[idx]);
      }
      
      const nextLevel: string[] = [];
      for (let i = 0; i < currentLevel.length; i += 2) {
        const left = currentLevel[i];
        const right = currentLevel[i + 1] || left;
        const combined = await cryptoHash(left + right);
        nextLevel.push(combined);
      }
      
      currentLevel = nextLevel;
      idx = Math.floor(idx / 2);
    }
    
    return proof;
  }

  clear(): void {
    this.receipts.clear();
    this.root = '';
  }

  toJSON(): { receipts: [string, unknown][] } {
    return {
      receipts: Array.from(this.receipts.entries())
    };
  }

  static async fromJSON(data: { receipts: [string, unknown][] }): Promise<ReceiptsTrie> {
    const trie = new ReceiptsTrie();
    for (const [key, value] of data.receipts) {
      trie.receipts.set(key, value);
    }
    await trie.updateRoot();
    return trie;
  }
}
