import type { DAGNode, SignedTransaction } from './types.js';
import { hashTransaction } from './crypto.js';

export class DAG {
  private nodes: Map<string, DAGNode> = new Map();
  private tips: Set<string> = new Set();

  constructor() {
    this.nodes = new Map();
    this.tips = new Set();
  }

  async addTransaction(tx: SignedTransaction): Promise<DAGNode> {
    const node: DAGNode = {
      tx,
      parents: tx.tips,
      children: [],
      weight: 0,
      confirmed: false
    };

    for (const parentHash of tx.tips) {
      const parent = this.nodes.get(parentHash);
      if (parent) {
        parent.children.push(tx.hash);
        this.tips.delete(parentHash);
      }
    }

    this.nodes.set(tx.hash, node);
    this.tips.add(tx.hash);

    return node;
  }

  getNode(hash: string): DAGNode | undefined {
    return this.nodes.get(hash);
  }

  getTips(): string[] {
    return Array.from(this.tips);
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
        for (const parentHash of node.parents) {
          visit(parentHash);
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
        for (const parent of node.parents) {
          if (!ancestors.has(parent)) {
            ancestors.add(parent);
            queue.push(parent);
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

  toJSON(): object {
    return {
      nodes: Array.from(this.nodes.entries()).map(([hash, node]) => ({
        hash,
        ...node
      })),
      tips: Array.from(this.tips)
    };
  }

  static fromJSON(data: any): DAG {
    const dag = new DAG();
    
    for (const nodeData of data.nodes) {
      const { hash, ...node } = nodeData;
      dag.nodes.set(hash, node);
    }

    dag.tips = new Set(data.tips);
    return dag;
  }
}
