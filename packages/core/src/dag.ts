import type { DAGNode, SignedTransaction } from './types.js';
import { hashTransaction } from './crypto.js';
import { parseTransactionURL, createTransactionURL } from './encoding.js';

export class DAG {
  private nodes: Map<string, DAGNode> = new Map();
  private tipHashes: Set<string> = new Set();
  private urlToHash: Map<string, string> = new Map();

  constructor() {
    this.nodes = new Map();
    this.tipHashes = new Set();
    this.urlToHash = new Map();
  }

  async addTransaction(tx: SignedTransaction): Promise<DAGNode> {
    const txUrl = createTransactionURL(tx).path;
    this.urlToHash.set(txUrl, tx.hash);

    const node: DAGNode = {
      tx,
      parentUrls: tx.tipUrls,
      children: [],
      weight: 0,
      confirmed: false
    };

    for (const parentUrl of tx.tipUrls) {
      const parentHash = this.resolveUrlToHash(parentUrl);
      if (parentHash) {
        const parent = this.nodes.get(parentHash);
        if (parent) {
          parent.children.push(tx.hash);
          this.tipHashes.delete(parentHash);
        }
      }
    }

    this.nodes.set(tx.hash, node);
    this.tipHashes.add(tx.hash);

    return node;
  }

  resolveUrlToHash(url: string): string | null {
    if (this.urlToHash.has(url)) {
      return this.urlToHash.get(url)!;
    }
    
    const tx = parseTransactionURL(url);
    if (tx) {
      for (const [hash, node] of this.nodes) {
        if (node.tx.from === tx.from && 
            node.tx.to === tx.to && 
            node.tx.amount === tx.amount &&
            node.tx.nonce === tx.nonce &&
            node.tx.ts === tx.ts) {
          this.urlToHash.set(url, hash);
          return hash;
        }
      }
    }
    
    return null;
  }

  getNode(hash: string): DAGNode | undefined {
    return this.nodes.get(hash);
  }

  getTips(): string[] {
    return Array.from(this.tipHashes);
  }

  getTipUrls(): string[] {
    return this.getTips().map(hash => {
      const node = this.nodes.get(hash);
      return node ? createTransactionURL(node.tx).path : '';
    }).filter(url => url !== '');
  }

  getAllNodes(): DAGNode[] {
    return Array.from(this.nodes.values());
  }

  selectTips(count: number = 2): string[] {
    const tipsList = this.getTips();
    
    if (tipsList.length === 0) {
      return [];
    }

    if (tipsList.length <= count) {
      return tipsList;
    }

    const selected: string[] = [];
    const available = [...tipsList];

    for (let i = 0; i < count && available.length > 0; i++) {
      const weights = available.map(hash => {
        const node = this.nodes.get(hash);
        return node ? node.weight + 1 : 1;
      });

      const totalWeight = weights.reduce((a, b) => a + b, 0);
      let random = Math.random() * totalWeight;

      for (let j = 0; j < available.length; j++) {
        random -= weights[j];
        if (random <= 0) {
          selected.push(available[j]);
          available.splice(j, 1);
          break;
        }
      }
    }

    return selected;
  }

  selectTipUrls(count: number = 2): string[] {
    const tipHashes = this.selectTips(count);
    return tipHashes.map(hash => {
      const node = this.nodes.get(hash);
      return node ? createTransactionURL(node.tx).path : '';
    }).filter(url => url !== '');
  }

  updateWeights(accountWeights: Map<string, number>): void {
    for (const [hash, node] of this.nodes) {
      const accountWeight = accountWeights.get(node.tx.from) || 0;
      node.weight = accountWeight;
    }

    const sorted = this.topologicalSort();
    for (const hash of sorted) {
      const node = this.nodes.get(hash)!;
      for (const childHash of node.children) {
        const child = this.nodes.get(childHash);
        if (child) {
          child.weight += node.weight;
        }
      }
    }
  }

  private topologicalSort(): string[] {
    const visited = new Set<string>();
    const result: string[] = [];

    const visit = (hash: string) => {
      if (visited.has(hash)) return;
      visited.add(hash);

      const node = this.nodes.get(hash);
      if (node) {
        for (const parentUrl of node.parentUrls) {
          const parentHash = this.resolveUrlToHash(parentUrl);
          if (parentHash) {
            visit(parentHash);
          }
        }
      }

      result.push(hash);
    };

    for (const hash of this.nodes.keys()) {
      visit(hash);
    }

    return result;
  }

  resolveConflict(tx1Hash: string, tx2Hash: string): string {
    const node1 = this.nodes.get(tx1Hash);
    const node2 = this.nodes.get(tx2Hash);

    if (!node1 || !node2) {
      throw new Error('Transaction not found');
    }

    const weight1 = this.getCumulativeWeight(tx1Hash);
    const weight2 = this.getCumulativeWeight(tx2Hash);

    return weight1 >= weight2 ? tx1Hash : tx2Hash;
  }

  private getCumulativeWeight(hash: string): number {
    const node = this.nodes.get(hash);
    if (!node) return 0;

    let weight = node.weight;
    for (const childHash of node.children) {
      weight += this.getCumulativeWeight(childHash);
    }

    return weight;
  }

  getAncestors(hash: string): Set<string> {
    const ancestors = new Set<string>();
    const queue = [hash];

    while (queue.length > 0) {
      const current = queue.shift()!;
      const node = this.nodes.get(current);

      if (node) {
        for (const parentUrl of node.parentUrls) {
          const parentHash = this.resolveUrlToHash(parentUrl);
          if (parentHash && !ancestors.has(parentHash)) {
            ancestors.add(parentHash);
            queue.push(parentHash);
          }
        }
      }
    }

    return ancestors;
  }

  getDescendants(hash: string): Set<string> {
    const descendants = new Set<string>();
    const queue = [hash];

    while (queue.length > 0) {
      const current = queue.shift()!;
      const node = this.nodes.get(current);

      if (node) {
        for (const child of node.children) {
          if (!descendants.has(child)) {
            descendants.add(child);
            queue.push(child);
          }
        }
      }
    }

    return descendants;
  }

  size(): number {
    return this.nodes.size;
  }

  pruneOldNodes(maxNodes: number): number {
    if (this.nodes.size <= maxNodes) return 0;

    const nodesToKeep = new Set<string>();
    
    for (const tipHash of this.tipHashes) {
      nodesToKeep.add(tipHash);
      const ancestors = this.getAncestors(tipHash);
      for (const ancestor of ancestors) {
        nodesToKeep.add(ancestor);
      }
    }

    if (nodesToKeep.size >= this.nodes.size) {
      return 0;
    }

    const toRemove: string[] = [];
    for (const hash of this.nodes.keys()) {
      if (!nodesToKeep.has(hash)) {
        toRemove.push(hash);
      }
    }

    for (const hash of toRemove) {
      const node = this.nodes.get(hash);
      if (node) {
        for (const [url, h] of this.urlToHash) {
          if (h === hash) {
            this.urlToHash.delete(url);
          }
        }
        for (const childHash of node.children) {
          const child = this.nodes.get(childHash);
          if (child && child.parentUrls) {
            child.parentUrls = child.parentUrls.filter(pUrl => {
              const parentHash = this.urlToHash.get(pUrl);
              return parentHash !== hash;
            });
          }
        }
      }
      this.nodes.delete(hash);
      this.tipHashes.delete(hash);
    }

    return toRemove.length;
  }

  toJSON(): object {
    return {
      nodes: Array.from(this.nodes.entries()).map(([hash, node]) => ({
        hash,
        ...node
      })),
      tipHashes: Array.from(this.tipHashes)
    };
  }

  static fromJSON(data: any): DAG {
    const dag = new DAG();
    
    for (const nodeData of data.nodes) {
      const { hash, ...node } = nodeData;
      if (node.parents && !node.parentUrls) {
        node.parentUrls = node.parents;
        delete node.parents;
      }
      dag.nodes.set(hash, node);
      
      if (node.tx?.url) {
        dag.urlToHash.set(node.tx.url, hash);
      }
    }

    dag.tipHashes = new Set(data.tipHashes || data.tips || []);
    
    return dag;
  }
}
