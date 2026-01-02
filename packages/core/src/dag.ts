import type { DAGNode, SignedTransaction, SelfCrawlableBundle, Checkpoint, FinalityMetadata, TruncatedParentRef } from './types.js';
import { hashTransaction } from './crypto.js';
import { parseTransactionURL, createTransactionURL, createSelfCrawlableURL } from './encoding.js';

function compactUrl(hash: string): string {
  return `/tx/h/${hash}`;
}

function extractHashFromUrl(url: string): string | null {
  const hashMatch = url.match(/\/tx\/h\/([a-f0-9]+)/);
  if (hashMatch) return hashMatch[1];
  
  const proofMatch = url.match(/\/txp\/(.+)/);
  if (proofMatch) {
    try {
      const decoded = Buffer.from(proofMatch[1], 'base64url');
      const { inflate } = require('pako');
      const json = JSON.parse(inflate(decoded, { to: 'string' }));
      if (json.hash) return json.hash;
    } catch {}
  }
  
  return null;
}

export class DAG {
  private nodes: Map<string, DAGNode> = new Map();
  private tipHashes: Set<string> = new Set();
  private urlToHash: Map<string, string> = new Map();
  private unresolvedParentCount = 0;

  constructor() {
    this.nodes = new Map();
    this.tipHashes = new Set();
    this.urlToHash = new Map();
  }

  async addTransaction(tx: SignedTransaction): Promise<DAGNode> {
    const txUrl = compactUrl(tx.hash);
    this.urlToHash.set(txUrl, tx.hash);

    const normalizedParentUrls: string[] = [];
    for (const parentUrl of tx.tipUrls) {
      const hash = extractHashFromUrl(parentUrl);
      if (hash && this.nodes.has(hash)) {
        normalizedParentUrls.push(compactUrl(hash));
        this.urlToHash.set(parentUrl, hash);
      } else {
        normalizedParentUrls.push(parentUrl);
      }
    }

    const node: DAGNode = {
      tx,
      parentUrls: normalizedParentUrls,
      children: [],
      weight: 0,
      confirmed: false,
      url: txUrl
    };

    for (const parentUrl of normalizedParentUrls) {
      const parentHash = this.resolveUrlToHash(parentUrl);
      if (parentHash) {
        const parent = this.nodes.get(parentHash);
        if (parent) {
          parent.children.push(tx.hash);
          this.tipHashes.delete(parentHash);
        }
      } else {
        this.unresolvedParentCount++;
      }
    }

    this.nodes.set(tx.hash, node);
    this.tipHashes.add(tx.hash);

    return node;
  }

  getStats(): { nodes: number; tips: number; unresolvedParents: number } {
    return {
      nodes: this.nodes.size,
      tips: this.tipHashes.size,
      unresolvedParents: this.unresolvedParentCount
    };
  }

  resolveUrlToHash(url: string): string | null {
    if (this.urlToHash.has(url)) {
      return this.urlToHash.get(url)!;
    }
    
    const hashMatch = url.match(/\/tx\/h\/([a-f0-9]+)/);
    if (hashMatch) {
      const hash = hashMatch[1];
      if (this.nodes.has(hash)) {
        this.urlToHash.set(url, hash);
        return hash;
      }
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
      return node?.url || '';
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
      return node?.url || '';
    }).filter(url => url !== '');
  }

  async buildSelfCrawlableBundle(
    hash: string,
    getCheckpoint?: (checkpointId: string) => { checkpointId: string; merkleRoot: string; txMerkleRoot?: string; height: number; signatureCount: number } | null,
    getMerkleProof?: (txHash: string, checkpointId: string) => Promise<{ proof: string[]; index: number; txMerkleRoot: string } | null>
  ): Promise<SelfCrawlableBundle | null> {
    const node = this.nodes.get(hash);
    if (!node) return null;

    const parents: SelfCrawlableBundle[] = [];
    const truncatedParents: TruncatedParentRef[] = [];

    for (const parentUrl of node.tx.tipUrls) {
      const parentHash = this.resolveUrlToHash(parentUrl);
      if (!parentHash) continue;
      
      const parentNode = this.nodes.get(parentHash);
      if (parentNode?.finality && getCheckpoint) {
        const checkpoint = getCheckpoint(parentNode.finality.checkpointId);
        if (checkpoint) {
          const truncatedRef: TruncatedParentRef = {
            hash: parentHash,
            tx: {
              from: parentNode.tx.from,
              to: parentNode.tx.to,
              amount: parentNode.tx.amount,
              nonce: parentNode.tx.nonce,
              tipUrls: parentNode.tx.tipUrls,
              sig: parentNode.tx.sig,
              ts: parentNode.tx.ts
            },
            checkpointAnchor: checkpoint
          };
          if (getMerkleProof && checkpoint.txMerkleRoot) {
            const proofResult = await getMerkleProof(parentHash, parentNode.finality.checkpointId);
            if (proofResult) {
              truncatedRef.merkleProof = {
                proof: proofResult.proof,
                index: proofResult.index,
                txMerkleRoot: proofResult.txMerkleRoot
              };
            }
          }
          truncatedParents.push(truncatedRef);
        }
        continue;
      }
      
      const parentBundle = await this.buildSelfCrawlableBundle(parentHash, getCheckpoint, getMerkleProof);
      if (parentBundle) {
        parents.push(parentBundle);
      }
    }

    const bundle: SelfCrawlableBundle = {
      tx: {
        from: node.tx.from,
        to: node.tx.to,
        amount: node.tx.amount,
        nonce: node.tx.nonce,
        tipUrls: node.tx.tipUrls,
        sig: node.tx.sig,
        ts: node.tx.ts
      },
      hash: node.tx.hash,
      parents
    };

    if (truncatedParents.length > 0) {
      bundle.truncatedParents = truncatedParents;
    }

    return bundle;
  }

  async getSelfCrawlableUrl(
    hash: string,
    getCheckpoint?: (checkpointId: string) => { checkpointId: string; merkleRoot: string; txMerkleRoot?: string; height: number; signatureCount: number } | null,
    getMerkleProof?: (txHash: string, checkpointId: string) => Promise<{ proof: string[]; index: number; txMerkleRoot: string } | null>
  ): Promise<string | null> {
    const bundle = await this.buildSelfCrawlableBundle(hash, getCheckpoint, getMerkleProof);
    if (!bundle) return null;
    return createSelfCrawlableURL(bundle).path;
  }

  updateWeights(accountWeights: Map<string, number>): void {
    for (const node of this.nodes.values()) {
      const accountWeight = accountWeights.get(node.tx.from) || 0;
      node.weight = accountWeight;
    }
  }

  setFinality(hash: string, checkpointId: string, checkpointHeight: number): boolean {
    const node = this.nodes.get(hash);
    if (!node) return false;
    
    if (node.finality) return false;
    
    node.finality = {
      checkpointId,
      checkpointHeight,
      finalizedAt: Date.now()
    };
    node.confirmed = true;
    return true;
  }

  stampFinalityForAll(checkpointId: string, checkpointHeight: number): number {
    let count = 0;
    for (const [hash, node] of this.nodes) {
      if (!node.finality) {
        node.finality = {
          checkpointId,
          checkpointHeight,
          finalizedAt: Date.now()
        };
        node.confirmed = true;
        count++;
      }
    }
    return count;
  }

  hasFinality(hash: string): boolean {
    const node = this.nodes.get(hash);
    return !!node?.finality;
  }

  getFinality(hash: string): FinalityMetadata | undefined {
    const node = this.nodes.get(hash);
    return node?.finality;
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

  pruneOldNodes(maxNodes: number): { hash: string; finality?: FinalityMetadata }[] {
    if (this.nodes.size <= maxNodes) return [];

    const allNodes = Array.from(this.nodes.entries());
    allNodes.sort((a, b) => b[1].tx.ts - a[1].tx.ts);
    
    const nodesToKeep = new Set<string>();
    for (let i = 0; i < Math.min(maxNodes, allNodes.length); i++) {
      nodesToKeep.add(allNodes[i][0]);
    }
    
    const toRemove: string[] = [];
    for (const hash of this.nodes.keys()) {
      if (!nodesToKeep.has(hash)) {
        toRemove.push(hash);
      }
    }

    const prunedNodes: { hash: string; finality?: FinalityMetadata }[] = [];

    for (const hash of toRemove) {
      const node = this.nodes.get(hash);
      if (node) {
        prunedNodes.push({ hash, finality: node.finality });
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

    return prunedNodes;
  }

  toJSON(): object {
    return {
      nodes: Array.from(this.nodes.entries()).map(([hash, node]) => ({
        hash,
        tx: {
          from: node.tx.from,
          to: node.tx.to,
          amount: node.tx.amount,
          nonce: node.tx.nonce,
          sig: node.tx.sig,
          ts: node.tx.ts,
          hash: node.tx.hash,
          parentHashes: node.tx.tipUrls.map(url => this.urlToHash.get(url) || url.slice(0, 20))
        },
        children: node.children,
        weight: node.weight,
        confirmed: node.confirmed,
        finality: node.finality
      })),
      tipHashes: Array.from(this.tipHashes)
    };
  }

  static fromJSON(data: any): DAG {
    const dag = new DAG();
    
    for (const nodeData of data.nodes) {
      const { hash, tx, children, weight, confirmed, finality } = nodeData;
      
      const compactUrl = `/tx/h/${hash}`;
      
      const restoredTx = {
        from: tx.from,
        to: tx.to,
        amount: tx.amount,
        nonce: tx.nonce,
        sig: tx.sig,
        ts: tx.ts,
        hash: tx.hash,
        tipUrls: [] as string[]
      };
      
      const node: DAGNode = {
        tx: restoredTx,
        parentUrls: [] as string[],
        children: children || [],
        weight: weight || 0,
        confirmed: confirmed || false,
        url: compactUrl,
        finality: finality
      };
      
      dag.nodes.set(hash, node);
      dag.urlToHash.set(compactUrl, hash);
    }

    for (const nodeData of data.nodes) {
      const node = dag.nodes.get(nodeData.hash);
      if (node && nodeData.tx.parentHashes) {
        node.tx.tipUrls = nodeData.tx.parentHashes.map((parentHash: string) => {
          const parentNode = dag.nodes.get(parentHash);
          return parentNode?.url || '';
        }).filter((url: string) => url !== '');
        node.parentUrls = node.tx.tipUrls;
      }
    }

    dag.tipHashes = new Set(data.tipHashes || data.tips || []);
    
    return dag;
  }
}
