import { StateManager } from './state.js';
import { Consensus } from './consensus.js';
import type { SignedTransaction } from '@rinku/core';
import { lookup } from 'dns/promises';

export interface SyncStatus {
  nodeId: string;
  merkleRoot: string;
  dagSize: number;
  tips: string[];
  tipUrls: string[];
  latestTs: number;
}

export interface PeerInfo {
  url: string;
  lastSeen: number;
  status: 'online' | 'offline' | 'unknown';
  merkleRoot?: string;
  nodeId?: string;
  discoveredFrom?: string;
}

export interface DiscoveryConfig {
  maxPeers: number;
  discoveryEnabled: boolean;
  announceEnabled: boolean;
}

export class PeerSyncService {
  private peers: Map<string, PeerInfo> = new Map();
  private state: StateManager;
  private consensus: Consensus;
  private nodeId: string;
  private syncInterval: NodeJS.Timeout | null = null;
  private onSync: (() => Promise<void>) | null = null;
  private selfUrl: string | null = null;
  private discoveryConfig: DiscoveryConfig = {
    maxPeers: 50,
    discoveryEnabled: true,
    announceEnabled: true
  };
  
  private rejectedTxCache: Map<string, number> = new Map();
  private static readonly MAX_REJECTED_CACHE = 10000;
  private static readonly REJECTED_TTL_MS = 5 * 60 * 1000;
  private rejectedCacheCleanupInterval: NodeJS.Timeout | null = null;

  constructor(state: StateManager, consensus: Consensus, nodeId: string) {
    this.state = state;
    this.consensus = consensus;
    this.nodeId = nodeId;
    
    this.rejectedCacheCleanupInterval = setInterval(() => {
      this.cleanupRejectedCache();
    }, 60000);
  }
  
  private cleanupRejectedCache(): void {
    const now = Date.now();
    const toDelete: string[] = [];
    
    for (const [hash, timestamp] of this.rejectedTxCache) {
      if (now - timestamp > PeerSyncService.REJECTED_TTL_MS) {
        toDelete.push(hash);
      }
    }
    
    for (const hash of toDelete) {
      this.rejectedTxCache.delete(hash);
    }
    
    if (this.rejectedTxCache.size > PeerSyncService.MAX_REJECTED_CACHE) {
      const entries = Array.from(this.rejectedTxCache.entries())
        .sort((a, b) => a[1] - b[1]);
      const toRemove = entries.slice(0, entries.length - PeerSyncService.MAX_REJECTED_CACHE);
      for (const [hash] of toRemove) {
        this.rejectedTxCache.delete(hash);
      }
    }
  }
  
  setSelfUrl(url: string): void {
    this.selfUrl = url;
  }

  setDiscoveryConfig(config: Partial<DiscoveryConfig>): void {
    this.discoveryConfig = { ...this.discoveryConfig, ...config };
  }

  getDiscoveryConfig(): DiscoveryConfig {
    return { ...this.discoveryConfig };
  }

  addPeer(url: string, discoveredFrom?: string): boolean {
    if (url === this.selfUrl) {
      return false;
    }
    
    if (!this.isValidPeerUrl(url)) {
      console.log(`Rejected invalid peer URL: ${url}`);
      return false;
    }
    
    if (this.peers.size >= this.discoveryConfig.maxPeers && !this.peers.has(url)) {
      return false;
    }
    
    if (!this.peers.has(url)) {
      this.peers.set(url, {
        url,
        lastSeen: 0,
        status: 'unknown',
        discoveredFrom
      });
      if (discoveredFrom) {
        console.log(`Discovered peer ${url} via ${discoveredFrom}`);
      }
      
      this.validatePeerDNS(url).catch(() => {
        this.peers.delete(url);
        console.log(`Removed peer ${url} after DNS validation failure`);
      });
      
      if (!this.syncInterval && this.peers.size === 1) {
        this.start();
      }
      
      return true;
    }
    return false;
  }

  private async validatePeerDNS(url: string): Promise<boolean> {
    try {
      const parsed = new URL(url);
      const hostname = parsed.hostname.replace(/^\[|\]$/g, '');
      
      if (this.isPrivateOrLoopbackIPv4(hostname) || this.isPrivateOrLoopbackIPv6(hostname)) {
        return false;
      }
      
      const result = await lookup(hostname, { all: true });
      
      for (const record of result) {
        const ip = record.address;
        if (this.isPrivateOrLoopbackIPv4(ip) || this.isPrivateOrLoopbackIPv6(ip)) {
          console.log(`DNS resolution for ${hostname} returned private/loopback IP: ${ip}`);
          throw new Error('DNS resolved to private/loopback address');
        }
      }
      
      return true;
    } catch (error) {
      throw error;
    }
  }

  private isValidPeerUrl(url: string): boolean {
    try {
      const parsed = new URL(url);
      
      if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
        return false;
      }
      
      let hostname = parsed.hostname.toLowerCase();
      
      if (hostname.startsWith('[') && hostname.endsWith(']')) {
        hostname = hostname.slice(1, -1);
      }
      
      if (hostname === 'localhost' || 
          hostname.endsWith('.localhost') ||
          hostname === '0.0.0.0' ||
          hostname.endsWith('.local') ||
          hostname.endsWith('.internal') ||
          hostname.endsWith('.localdomain')) {
        return false;
      }
      
      if (this.isPrivateOrLoopbackIPv4(hostname)) {
        return false;
      }
      
      if (this.isPrivateOrLoopbackIPv6(hostname)) {
        return false;
      }
      
      if (/^\d+$/.test(hostname) || /^0x[0-9a-f]+$/i.test(hostname)) {
        return false;
      }
      
      return true;
    } catch {
      return false;
    }
  }

  private isPrivateOrLoopbackIPv4(hostname: string): boolean {
    const ipv4Match = hostname.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/);
    if (!ipv4Match) {
      return false;
    }
    
    const octets = ipv4Match.slice(1, 5).map(Number);
    if (octets.some(o => o < 0 || o > 255)) {
      return false;
    }
    
    const [a, b, c, d] = octets;
    
    if (a === 127) return true;
    if (a === 10) return true;
    if (a === 172 && b >= 16 && b <= 31) return true;
    if (a === 192 && b === 168) return true;
    if (a === 169 && b === 254) return true;
    if (a === 0) return true;
    if (a === 100 && b >= 64 && b <= 127) return true;
    if (a === 198 && b >= 18 && b <= 19) return true;
    if (a === 192 && b === 0 && c === 0) return true;
    if (a === 192 && b === 0 && c === 2) return true;
    if (a === 198 && b === 51 && c === 100) return true;
    if (a === 203 && b === 0 && c === 113) return true;
    if (a >= 224) return true;
    if (a === 255 && b === 255 && c === 255 && d === 255) return true;
    
    return false;
  }

  private isPrivateOrLoopbackIPv6(hostname: string): boolean {
    if (!hostname.includes(':')) {
      return false;
    }
    
    const lower = hostname.toLowerCase();
    
    if (lower === '::' || lower === '::1') return true;
    if (lower.startsWith('::ffff:')) return true;
    if (lower.startsWith('fc') || lower.startsWith('fd')) return true;
    if (lower.startsWith('fe80')) return true;
    if (lower.startsWith('ff')) return true;
    if (lower.startsWith('100::')) return true;
    if (lower.startsWith('2001:db8')) return true;
    if (lower.startsWith('2001:0db8')) return true;
    
    return false;
  }

  removePeer(url: string): void {
    this.peers.delete(url);
  }

  getPeers(): PeerInfo[] {
    return Array.from(this.peers.values());
  }

  getPeerCount(): number {
    return this.peers.size;
  }

  getOnlinePeers(): PeerInfo[] {
    return Array.from(this.peers.values()).filter(p => p.status === 'online');
  }

  getStatus(): SyncStatus {
    const nodes = this.consensus.getAllNodes();
    const latestTs = nodes.reduce((max, n) => Math.max(max, n.tx.ts), 0);
    
    return {
      nodeId: this.nodeId,
      merkleRoot: this.state.getMerkleRoot(),
      dagSize: nodes.length,
      tips: this.consensus.getTips(),
      tipUrls: this.consensus.getTipUrls(),
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
    if (this.rejectedCacheCleanupInterval) {
      clearInterval(this.rejectedCacheCleanupInterval);
      this.rejectedCacheCleanupInterval = null;
    }
  }

  private async syncWithPeers(): Promise<void> {
    for (const [url, peer] of this.peers) {
      try {
        const status = await this.fetchPeerStatus(url);
        peer.status = 'online';
        peer.lastSeen = Date.now();
        peer.merkleRoot = status.merkleRoot;
        peer.nodeId = status.nodeId;

        if (status.merkleRoot !== this.state.getMerkleRoot()) {
          console.log(`Merkle mismatch with ${url}, syncing...`);
          await this.syncFromPeer(url, status);
        }

        if (this.discoveryConfig.discoveryEnabled) {
          await this.discoverPeersFromPeer(url);
        }
      } catch (err) {
        peer.status = 'offline';
      }
    }
  }

  private async discoverPeersFromPeer(peerUrl: string): Promise<number> {
    try {
      const response = await fetch(`${peerUrl}/api/sync/peers`, {
        signal: AbortSignal.timeout(5000)
      });
      
      if (!response.ok) {
        return 0;
      }

      const data = await response.json() as { peers: PeerInfo[] };
      let discovered = 0;

      for (const remotePeer of data.peers) {
        if (remotePeer.url && remotePeer.status === 'online') {
          if (this.addPeer(remotePeer.url, peerUrl)) {
            discovered++;
          }
        }
      }

      return discovered;
    } catch {
      return 0;
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
      
      if (this.rejectedTxCache.has(item.tx.hash)) {
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
          this.rejectedTxCache.set(item.tx.hash, Date.now());
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

      for (const parentUrl of item.tx.tipUrls) {
        for (const [h, t] of txMap) {
          if (t.tx.tipUrls.some(u => u === parentUrl) || existingHashes.has(h)) {
            visit(h);
          }
        }
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
