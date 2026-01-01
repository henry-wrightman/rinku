import { StateManager } from './state.js';
import { Consensus } from './consensus.js';
import type { SignedTransaction } from '@rinku/core';

export interface SyncStatus {
  nodeId: string;
  merkleRoot: string;
  dagSize: number;
  tips: string[];
  latestTs: number;
}

export interface PeerInfo {
  url: string;
  lastSeen: number;
  status: 'online' | 'offline' | 'unknown';
  merkleRoot?: string;
}

export class PeerSyncService {
  private peers: Map<string, PeerInfo> = new Map();
  private state: StateManager;
  private consensus: Consensus;
  private nodeId: string;
  private syncInterval: NodeJS.Timeout | null = null;
  private onSync: (() => Promise<void>) | null = null;

  constructor(state: StateManager, consensus: Consensus, nodeId: string) {
    this.state = state;
    this.consensus = consensus;
    this.nodeId = nodeId;
  }

  addPeer(url: string): void {
    if (!this.peers.has(url)) {
      this.peers.set(url, {
        url,
        lastSeen: 0,
        status: 'unknown'
      });
    }
  }

  removePeer(url: string): void {
    this.peers.delete(url);
  }

  getPeers(): PeerInfo[] {
    return Array.from(this.peers.values());
  }

  getStatus(): SyncStatus {
    const nodes = this.consensus.getAllNodes();
    const latestTs = nodes.reduce((max, n) => Math.max(max, n.tx.ts), 0);
    
    return {
      nodeId: this.nodeId,
      merkleRoot: this.state.getMerkleRoot(),
      dagSize: nodes.length,
      tips: this.consensus.getTips(),
      latestTs
    };
  }

  onSyncComplete(callback: () => Promise<void>): void {
    this.onSync = callback;
  }

  start(intervalMs: number = 5000): void {
    if (this.syncInterval) {
      return;
    }

    this.syncInterval = setInterval(() => this.syncWithPeers(), intervalMs);
    console.log(`Peer sync started (interval: ${intervalMs}ms)`);
  }

  stop(): void {
    if (this.syncInterval) {
      clearInterval(this.syncInterval);
      this.syncInterval = null;
    }
  }

  private async syncWithPeers(): Promise<void> {
    for (const [url, peer] of this.peers) {
      try {
        const status = await this.fetchPeerStatus(url);
        peer.status = 'online';
        peer.lastSeen = Date.now();
        peer.merkleRoot = status.merkleRoot;

        if (status.merkleRoot !== this.state.getMerkleRoot()) {
          console.log(`Merkle mismatch with ${url}, syncing...`);
          await this.syncFromPeer(url, status);
        }
      } catch (err) {
        peer.status = 'offline';
      }
    }
  }

  private async fetchPeerStatus(url: string): Promise<SyncStatus> {
    const response = await fetch(`${url}/api/sync/status`, {
      signal: AbortSignal.timeout(5000)
    });
    if (!response.ok) {
      throw new Error('Failed to fetch peer status');
    }
    return response.json() as Promise<SyncStatus>;
  }

  private async syncFromPeer(url: string, peerStatus: SyncStatus): Promise<void> {
    const ourHashes = new Set(this.consensus.getAllNodes().map(n => n.tx.hash));
    
    const response = await fetch(`${url}/api/sync/transactions`, {
      signal: AbortSignal.timeout(30000)
    });
    
    if (!response.ok) {
      throw new Error('Failed to fetch transactions');
    }

    const data = await response.json() as { 
      transactions: { tx: SignedTransaction; publicKey?: number[] }[] 
    };

    const txMap = new Map<string, { tx: SignedTransaction; publicKey?: number[] }>();
    for (const item of data.transactions) {
      if (!ourHashes.has(item.tx.hash)) {
        txMap.set(item.tx.hash, item);
      }
    }

    if (txMap.size === 0) {
      return;
    }

    const sorted = this.topologicalSort(txMap, ourHashes);
    let synced = 0;

    for (const hash of sorted) {
      const item = txMap.get(hash);
      if (!item) continue;

      if (this.consensus.hasTransaction(item.tx.hash)) {
        continue;
      }

      const pubKeyArray = item.publicKey ? new Uint8Array(item.publicKey) : undefined;
      
      if (pubKeyArray) {
        this.consensus.registerPublicKey(item.tx.from, pubKeyArray);
      }

      const skipValidation = item.tx.from === 'genesis' || item.tx.from === 'faucet';
      
      if (!skipValidation) {
        const validation = await this.consensus.validateTransaction(
          item.tx,
          this.state.getAllAccounts(),
          pubKeyArray
        );

        if (!validation.valid) {
          console.log(`Skipping invalid tx ${item.tx.hash}: ${validation.error}`);
          continue;
        }
      }

      await this.state.applyTransaction(item.tx, { skipChecks: skipValidation });
      await this.consensus.addTransaction(item.tx);
      synced++;
    }

    if (synced > 0) {
      this.consensus.updateWeights(this.state.getAllAccounts());
      console.log(`Synced ${synced} transactions from ${url}`);
      
      if (this.onSync) {
        await this.onSync();
      }
    }
  }

  private topologicalSort(
    txMap: Map<string, { tx: SignedTransaction; publicKey?: number[] }>,
    existingHashes: Set<string>
  ): string[] {
    const result: string[] = [];
    const visited = new Set<string>();
    const visiting = new Set<string>();

    const visit = (hash: string) => {
      if (visited.has(hash) || existingHashes.has(hash)) {
        return;
      }
      if (visiting.has(hash)) {
        return;
      }

      const item = txMap.get(hash);
      if (!item) return;

      visiting.add(hash);

      for (const parentHash of item.tx.tips) {
        visit(parentHash);
      }

      visiting.delete(hash);
      visited.add(hash);
      result.push(hash);
    };

    for (const hash of txMap.keys()) {
      visit(hash);
    }

    return result;
  }

  async forceSync(): Promise<{ synced: number; errors: string[] }> {
    let totalSynced = 0;
    const errors: string[] = [];

    for (const [url, peer] of this.peers) {
      try {
        const status = await this.fetchPeerStatus(url);
        peer.status = 'online';
        peer.lastSeen = Date.now();

        if (status.merkleRoot !== this.state.getMerkleRoot()) {
          await this.syncFromPeer(url, status);
          totalSynced++;
        }
      } catch (err: any) {
        peer.status = 'offline';
        errors.push(`${url}: ${err.message}`);
      }
    }

    return { synced: totalSynced, errors };
  }
}
