import type { SignedTransaction, DAGNode } from '@rinku/core';
import { Consensus } from './consensus.js';
import { StateManager } from './state.js';
import { GossipService } from './gossip.js';

export interface DoubleSpendInfo {
  account: string;
  nonce: number;
  txHash1: string;
  txHash2: string;
  detectedAt: number;
  resolvedAt?: number;
  winnerHash?: string;
}

export interface BranchInfo {
  tipHash: string;
  cumulativeWeight: number;
  transactionCount: number;
  latestTimestamp: number;
  containsDoubleSpend: boolean;
}

export interface ForkEvent {
  forkId: string;
  commonAncestor: string | null;
  branches: BranchInfo[];
  detectedAt: number;
  resolvedAt?: number;
  winningBranch?: string;
}

export interface ForkRemediationConfig {
  doubleSpendCheckIntervalMs: number;
  branchPruningEnabled: boolean;
  minWeightAdvantageForPruning: number;
  maxUnfinalizedTxAge: number;
  logSummaryIntervalMs: number;
  verboseLogging: boolean;
}

const DEFAULT_CONFIG: ForkRemediationConfig = {
  doubleSpendCheckIntervalMs: 2000,
  branchPruningEnabled: true,
  minWeightAdvantageForPruning: 0.2,
  maxUnfinalizedTxAge: 60000,
  logSummaryIntervalMs: 30000,
  verboseLogging: false,
};

export class ForkRemediationService {
  private consensus: Consensus;
  private state: StateManager;
  private gossip: GossipService | null = null;
  private config: ForkRemediationConfig;
  
  private doubleSpends: Map<string, DoubleSpendInfo> = new Map();
  private prunedBranches: Map<string, Set<string>> = new Map();
  private forkEvents: Map<string, ForkEvent> = new Map();
  private nonceIndex: Map<string, Map<number, string[]>> = new Map();
  
  private checkInterval: NodeJS.Timeout | null = null;
  private summaryInterval: NodeJS.Timeout | null = null;
  
  private detectionsSinceLastSummary = 0;
  private resolutionsSinceLastSummary = 0;
  private prunesSinceLastSummary = 0;

  constructor(
    consensus: Consensus,
    state: StateManager,
    config: Partial<ForkRemediationConfig> = {}
  ) {
    this.consensus = consensus;
    this.state = state;
    this.config = { ...DEFAULT_CONFIG, ...config };
  }

  setGossipService(gossip: GossipService): void {
    this.gossip = gossip;
    
    this.gossip.setConflictResolvedCallback(async (conflictId, winnerHash, loserHash) => {
      await this.handleConflictResolution(conflictId, winnerHash, loserHash);
    });
  }

  start(): void {
    if (this.checkInterval) return;
    
    this.checkInterval = setInterval(
      () => this.runDoubleSpendCheck(),
      this.config.doubleSpendCheckIntervalMs
    );
    
    this.summaryInterval = setInterval(
      () => this.logSummary(),
      this.config.logSummaryIntervalMs
    );
    
    console.log('Fork remediation service started');
  }

  stop(): void {
    if (this.checkInterval) {
      clearInterval(this.checkInterval);
      this.checkInterval = null;
    }
    if (this.summaryInterval) {
      clearInterval(this.summaryInterval);
      this.summaryInterval = null;
    }
  }
  
  private logSummary(): void {
    const stats = this.getStats();
    if (this.detectionsSinceLastSummary > 0 || this.resolutionsSinceLastSummary > 0 || stats.activeForks > 0) {
      console.log(`[Fork] Summary: ${this.detectionsSinceLastSummary} detected, ${this.resolutionsSinceLastSummary} resolved, ${this.prunesSinceLastSummary} pruned | Active: ${stats.activeForks} forks, ${stats.activeDoubleSpends} double-spends`);
    }
    this.detectionsSinceLastSummary = 0;
    this.resolutionsSinceLastSummary = 0;
    this.prunesSinceLastSummary = 0;
  }

  indexTransaction(tx: SignedTransaction): void {
    if (!this.nonceIndex.has(tx.from)) {
      this.nonceIndex.set(tx.from, new Map());
    }
    
    const accountNonces = this.nonceIndex.get(tx.from)!;
    if (!accountNonces.has(tx.nonce)) {
      accountNonces.set(tx.nonce, []);
    }
    
    const txList = accountNonces.get(tx.nonce)!;
    if (!txList.includes(tx.hash)) {
      txList.push(tx.hash);
      
      if (txList.length > 1) {
        this.detectDoubleSpend(tx.from, tx.nonce, txList);
      }
    }
  }

  private detectDoubleSpend(account: string, nonce: number, txHashes: string[]): void {
    for (let i = 0; i < txHashes.length; i++) {
      for (let j = i + 1; j < txHashes.length; j++) {
        const key = [txHashes[i], txHashes[j]].sort().join(':');
        
        if (!this.doubleSpends.has(key)) {
          const info: DoubleSpendInfo = {
            account,
            nonce,
            txHash1: txHashes[i],
            txHash2: txHashes[j],
            detectedAt: Date.now()
          };
          
          this.doubleSpends.set(key, info);
          this.detectionsSinceLastSummary++;
          if (this.config.verboseLogging) {
            console.log(`[Fork] Double-spend detected: account=${account.slice(0, 16)}..., nonce=${nonce}`);
          }
          
          if (this.gossip) {
            this.gossip.detectConflict(txHashes[i], txHashes[j], account);
          } else {
            this.resolveDoubleSpendLocally(info);
          }
        }
      }
    }
  }

  private resolveDoubleSpendLocally(info: DoubleSpendInfo): void {
    const weight1 = this.getCumulativeWeight(info.txHash1);
    const weight2 = this.getCumulativeWeight(info.txHash2);
    
    const winnerHash = weight1 >= weight2 ? info.txHash1 : info.txHash2;
    const loserHash = weight1 >= weight2 ? info.txHash2 : info.txHash1;
    
    info.winnerHash = winnerHash;
    info.resolvedAt = Date.now();
    this.resolutionsSinceLastSummary++;
    
    if (this.config.verboseLogging) {
      console.log(`[Fork] Double-spend resolved locally: winner=${winnerHash.slice(0, 16)}...`);
    }
    
    if (this.config.branchPruningEnabled) {
      this.pruneBranch(loserHash);
    }
  }

  private async handleConflictResolution(conflictId: string, winnerHash: string, loserHash: string): Promise<void> {
    const info = this.doubleSpends.get(conflictId);
    if (info) {
      info.winnerHash = winnerHash;
      info.resolvedAt = Date.now();
    }
    
    if (this.config.branchPruningEnabled) {
      this.pruneBranch(loserHash);
    }
  }

  private pruneBranch(txHash: string): void {
    const descendants = this.getDescendants(txHash);
    descendants.add(txHash);
    
    this.prunedBranches.set(txHash, descendants);
    this.prunesSinceLastSummary++;
    
    if (this.config.verboseLogging) {
      console.log(`[Fork] Pruned branch starting at ${txHash.slice(0, 16)}... (${descendants.size} transactions)`);
    }
  }

  isPruned(txHash: string): boolean {
    for (const prunedSet of this.prunedBranches.values()) {
      if (prunedSet.has(txHash)) {
        return true;
      }
    }
    return false;
  }

  private getDescendants(txHash: string): Set<string> {
    const descendants = new Set<string>();
    const node = this.consensus.getNode(txHash);
    if (!node) return descendants;
    
    const queue = [...node.children];
    while (queue.length > 0) {
      const childHash = queue.shift()!;
      if (descendants.has(childHash)) continue;
      
      descendants.add(childHash);
      const childNode = this.consensus.getNode(childHash);
      if (childNode) {
        queue.push(...childNode.children);
      }
    }
    
    return descendants;
  }

  private getCumulativeWeight(txHash: string): number {
    const node = this.consensus.getNode(txHash);
    if (!node) return 0;
    
    let weight = node.weight;
    const descendants = this.getDescendants(txHash);
    
    for (const descHash of descendants) {
      const descNode = this.consensus.getNode(descHash);
      if (descNode) {
        weight += descNode.weight;
      }
    }
    
    return weight;
  }

  private runDoubleSpendCheck(): void {
    const nodes = this.consensus.getAllNodes();
    
    for (const node of nodes) {
      this.indexTransaction(node.tx);
    }
    
    this.detectForks();
    
    this.cleanupOldData();
  }

  private detectForks(): void {
    const tips = this.consensus.getTips();
    if (tips.length <= 1) return;
    
    for (let i = 0; i < tips.length; i++) {
      for (let j = i + 1; j < tips.length; j++) {
        const tip1 = tips[i];
        const tip2 = tips[j];
        
        const ancestors1 = this.getAncestors(tip1);
        const ancestors2 = this.getAncestors(tip2);
        
        let commonAncestor: string | null = null;
        for (const ancestor of ancestors1) {
          if (ancestors2.has(ancestor)) {
            commonAncestor = ancestor;
            break;
          }
        }
        
        if (!commonAncestor) continue;
        
        const conflictingNonces = this.findConflictingNonces(tip1, tip2, commonAncestor);
        if (conflictingNonces.length === 0) continue;
        
        const forkId = [tip1, tip2].sort().join(':');
        if (!this.forkEvents.has(forkId)) {
          const branch1 = this.analyzeBranch(tip1, commonAncestor);
          const branch2 = this.analyzeBranch(tip2, commonAncestor);
          
          this.forkEvents.set(forkId, {
            forkId,
            commonAncestor,
            branches: [branch1, branch2],
            detectedAt: Date.now()
          });
          
          this.detectionsSinceLastSummary++;
          if (this.config.verboseLogging) {
            console.log(`[Fork] Fork detected: ${forkId.slice(0, 32)}... with ${conflictingNonces.length} conflicts`);
          }
          
          this.attemptForkResolution(forkId);
        }
      }
    }
  }

  private getAncestors(txHash: string): Set<string> {
    const ancestors = new Set<string>();
    const queue = [txHash];
    
    while (queue.length > 0) {
      const current = queue.shift()!;
      const node = this.consensus.getNode(current);
      if (!node) continue;
      
      for (const parentUrl of node.parentUrls) {
        const parentHash = this.resolveUrlToHash(parentUrl);
        if (parentHash && !ancestors.has(parentHash)) {
          ancestors.add(parentHash);
          queue.push(parentHash);
        }
      }
    }
    
    return ancestors;
  }

  private resolveUrlToHash(url: string): string | null {
    const hashMatch = url.match(/\/tx\/h\/([a-f0-9]+)/);
    return hashMatch ? hashMatch[1] : null;
  }

  private findConflictingNonces(tip1: string, tip2: string, commonAncestor: string): Array<{ account: string; nonce: number }> {
    const conflicts: Array<{ account: string; nonce: number }> = [];
    
    const branch1Txs = this.getBranchTransactions(tip1, commonAncestor);
    const branch2Txs = this.getBranchTransactions(tip2, commonAncestor);
    
    const branch1Nonces = new Map<string, Set<number>>();
    for (const tx of branch1Txs) {
      if (!branch1Nonces.has(tx.from)) {
        branch1Nonces.set(tx.from, new Set());
      }
      branch1Nonces.get(tx.from)!.add(tx.nonce);
    }
    
    for (const tx of branch2Txs) {
      if (branch1Nonces.has(tx.from) && branch1Nonces.get(tx.from)!.has(tx.nonce)) {
        conflicts.push({ account: tx.from, nonce: tx.nonce });
      }
    }
    
    return conflicts;
  }

  private getBranchTransactions(tipHash: string, stopAt: string): SignedTransaction[] {
    const transactions: SignedTransaction[] = [];
    const visited = new Set<string>();
    const queue = [tipHash];
    
    while (queue.length > 0) {
      const current = queue.shift()!;
      if (visited.has(current) || current === stopAt) continue;
      
      visited.add(current);
      const node = this.consensus.getNode(current);
      if (!node) continue;
      
      transactions.push(node.tx);
      
      for (const parentUrl of node.parentUrls) {
        const parentHash = this.resolveUrlToHash(parentUrl);
        if (parentHash) {
          queue.push(parentHash);
        }
      }
    }
    
    return transactions;
  }

  private analyzeBranch(tipHash: string, commonAncestor: string): BranchInfo {
    const transactions = this.getBranchTransactions(tipHash, commonAncestor);
    let cumulativeWeight = 0;
    let latestTimestamp = 0;
    let containsDoubleSpend = false;
    
    for (const tx of transactions) {
      const node = this.consensus.getNode(tx.hash);
      if (node) {
        cumulativeWeight += node.weight;
        latestTimestamp = Math.max(latestTimestamp, tx.ts);
      }
      
      const accountNonces = this.nonceIndex.get(tx.from);
      if (accountNonces) {
        const txList = accountNonces.get(tx.nonce);
        if (txList && txList.length > 1) {
          containsDoubleSpend = true;
        }
      }
    }
    
    return {
      tipHash,
      cumulativeWeight,
      transactionCount: transactions.length,
      latestTimestamp,
      containsDoubleSpend
    };
  }

  private attemptForkResolution(forkId: string): void {
    const fork = this.forkEvents.get(forkId);
    if (!fork || fork.resolvedAt) return;
    
    const [branch1, branch2] = fork.branches;
    const totalWeight = branch1.cumulativeWeight + branch2.cumulativeWeight;
    
    if (totalWeight === 0) return;
    
    const weight1Ratio = branch1.cumulativeWeight / totalWeight;
    const weight2Ratio = branch2.cumulativeWeight / totalWeight;
    
    const threshold = 0.5 + this.config.minWeightAdvantageForPruning;
    
    if (weight1Ratio >= threshold) {
      fork.winningBranch = branch1.tipHash;
      fork.resolvedAt = Date.now();
      if (this.config.branchPruningEnabled) {
        this.pruneBranch(branch2.tipHash);
      }
      this.resolutionsSinceLastSummary++;
      if (this.config.verboseLogging) {
        console.log(`[Fork] Fork ${forkId.slice(0, 16)}... resolved: branch1 wins (${(weight1Ratio * 100).toFixed(1)}%)`);
      }
    } else if (weight2Ratio >= threshold) {
      fork.winningBranch = branch2.tipHash;
      fork.resolvedAt = Date.now();
      if (this.config.branchPruningEnabled) {
        this.pruneBranch(branch1.tipHash);
      }
      this.resolutionsSinceLastSummary++;
      if (this.config.verboseLogging) {
        console.log(`[Fork] Fork ${forkId.slice(0, 16)}... resolved: branch2 wins (${(weight2Ratio * 100).toFixed(1)}%)`);
      }
    }
  }

  private cleanupOldData(): void {
    const now = Date.now();
    const maxAge = 300000;
    
    for (const [key, info] of this.doubleSpends) {
      if (info.resolvedAt && now - info.resolvedAt > maxAge) {
        this.doubleSpends.delete(key);
      }
    }
    
    for (const [key, fork] of this.forkEvents) {
      if (fork.resolvedAt && now - fork.resolvedAt > maxAge) {
        this.forkEvents.delete(key);
      }
    }
  }

  getStats(): {
    activeDoubleSpends: number;
    resolvedDoubleSpends: number;
    activeForks: number;
    resolvedForks: number;
    prunedBranches: number;
  } {
    const doubleSpendsList = Array.from(this.doubleSpends.values());
    const forksList = Array.from(this.forkEvents.values());
    
    return {
      activeDoubleSpends: doubleSpendsList.filter(d => !d.resolvedAt).length,
      resolvedDoubleSpends: doubleSpendsList.filter(d => d.resolvedAt).length,
      activeForks: forksList.filter(f => !f.resolvedAt).length,
      resolvedForks: forksList.filter(f => f.resolvedAt).length,
      prunedBranches: this.prunedBranches.size
    };
  }

  getDoubleSpends(): DoubleSpendInfo[] {
    return Array.from(this.doubleSpends.values());
  }

  getForkEvents(): ForkEvent[] {
    return Array.from(this.forkEvents.values());
  }

  toJSON(): object {
    return {
      doubleSpends: Array.from(this.doubleSpends.entries()),
      prunedBranches: Array.from(this.prunedBranches.entries()).map(([k, v]) => [k, Array.from(v)]),
      forkEvents: Array.from(this.forkEvents.entries()).map(([k, v]) => [k, {
        ...v,
        branches: v.branches
      }]),
      nonceIndex: Array.from(this.nonceIndex.entries()).map(([account, nonces]) => [
        account,
        Array.from(nonces.entries())
      ])
    };
  }

  static fromJSON(
    data: any,
    consensus: Consensus,
    state: StateManager,
    config?: Partial<ForkRemediationConfig>
  ): ForkRemediationService {
    const service = new ForkRemediationService(consensus, state, config);
    
    if (data.doubleSpends) {
      service.doubleSpends = new Map(data.doubleSpends);
    }
    
    if (data.prunedBranches) {
      service.prunedBranches = new Map(
        data.prunedBranches.map(([k, v]: [string, string[]]) => [k, new Set(v)])
      );
    }
    
    if (data.forkEvents) {
      service.forkEvents = new Map(data.forkEvents);
    }
    
    if (data.nonceIndex) {
      for (const [account, nonces] of data.nonceIndex) {
        service.nonceIndex.set(account, new Map(nonces));
      }
    }
    
    return service;
  }
}
