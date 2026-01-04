import type { SignedTransaction, Checkpoint } from '@rinku/core';
import { Consensus } from './consensus.js';
import { StateManager } from './state.js';
import { PeerSyncService, PeerInfo } from './peerSync.js';

export interface GossipMessage {
  type: 'tx' | 'tip_announce' | 'checkpoint_sig' | 'validator_set' | 'conflict_resolution';
  nodeId: string;
  timestamp: number;
  payload: unknown;
}

export interface TxGossipPayload {
  tx: SignedTransaction;
  publicKey?: number[];
}

export interface TipAnnouncePayload {
  tips: string[];
  tipUrls: string[];
  dagSize: number;
  merkleRoot: string;
}

export interface CheckpointSigPayload {
  checkpointId: string;
  height: number;
  validatorAddress: string;
  signature: string;
  publicKey: number[];
  weight: number;
}

export interface ValidatorSetPayload {
  validators: Array<{
    address: string;
    publicKey: number[];
    weight: number;
    registeredAt: number;
  }>;
  merkleRoot: string;
  timestamp: number;
}

export interface ConflictResolutionPayload {
  conflictId: string;
  txHash1: string;
  txHash2: string;
  winnerHash: string;
  cumulativeWeight1: number;
  cumulativeWeight2: number;
  resolvedBy: string;
  timestamp: number;
}

export interface PendingCheckpointSig {
  checkpointId: string;
  height: number;
  signatures: Map<string, {
    signature: string;
    publicKey: Uint8Array;
    weight: number;
    receivedAt: number;
  }>;
  totalWeight: number;
  requiredWeight: number;
}

export interface ConflictRecord {
  conflictId: string;
  txHash1: string;
  txHash2: string;
  account: string;
  detectedAt: number;
  resolvedAt?: number;
  winnerHash?: string;
  votes: Map<string, { voterWeight: number; votedFor: string }>;
}

export interface GossipConfig {
  gossipIntervalMs: number;
  txBatchSize: number;
  maxPendingTxs: number;
  conflictResolutionThreshold: number;
  checkpointQuorumPercent: number;
}

const DEFAULT_GOSSIP_CONFIG: GossipConfig = {
  gossipIntervalMs: 1000,
  txBatchSize: 50,
  maxPendingTxs: 1000,
  conflictResolutionThreshold: 0.67,
  checkpointQuorumPercent: 67,
};

export class GossipService {
  private consensus: Consensus;
  private state: StateManager;
  private peerSync: PeerSyncService;
  private nodeId: string;
  private config: GossipConfig;
  
  private pendingTxs: Map<string, TxGossipPayload> = new Map();
  private seenTxHashes: Set<string> = new Set();
  private pendingCheckpointSigs: Map<string, PendingCheckpointSig> = new Map();
  private conflicts: Map<string, ConflictRecord> = new Map();
  private resolvedConflicts: Map<string, string> = new Map();
  private validatorSet: Map<string, { publicKey: Uint8Array; weight: number; registeredAt: number }> = new Map();
  private validatorSetMerkle: string = '';
  
  private gossipInterval: NodeJS.Timeout | null = null;
  private onTxReceived: ((tx: SignedTransaction, publicKey?: Uint8Array) => Promise<boolean>) | null = null;
  private onCheckpointQuorum: ((checkpointId: string, signatures: Map<string, { signature: string; publicKey: Uint8Array; weight: number }>) => Promise<void>) | null = null;
  private onConflictResolved: ((conflictId: string, winnerHash: string, loserHash: string) => Promise<void>) | null = null;

  constructor(
    consensus: Consensus,
    state: StateManager,
    peerSync: PeerSyncService,
    nodeId: string,
    config: Partial<GossipConfig> = {}
  ) {
    this.consensus = consensus;
    this.state = state;
    this.peerSync = peerSync;
    this.nodeId = nodeId;
    this.config = { ...DEFAULT_GOSSIP_CONFIG, ...config };
  }

  setTxReceivedCallback(cb: (tx: SignedTransaction, publicKey?: Uint8Array) => Promise<boolean>): void {
    this.onTxReceived = cb;
  }

  setCheckpointQuorumCallback(cb: (checkpointId: string, signatures: Map<string, { signature: string; publicKey: Uint8Array; weight: number }>) => Promise<void>): void {
    this.onCheckpointQuorum = cb;
  }

  setConflictResolvedCallback(cb: (conflictId: string, winnerHash: string, loserHash: string) => Promise<void>): void {
    this.onConflictResolved = cb;
  }

  start(): void {
    if (this.gossipInterval) return;
    
    this.gossipInterval = setInterval(() => this.gossipRound(), this.config.gossipIntervalMs);
    console.log(`Gossip service started (interval: ${this.config.gossipIntervalMs}ms)`);
  }

  stop(): void {
    if (this.gossipInterval) {
      clearInterval(this.gossipInterval);
      this.gossipInterval = null;
    }
  }

  broadcastTransaction(tx: SignedTransaction, publicKey?: Uint8Array): void {
    if (this.seenTxHashes.has(tx.hash)) return;
    
    this.seenTxHashes.add(tx.hash);
    this.pendingTxs.set(tx.hash, {
      tx,
      publicKey: publicKey ? Array.from(publicKey) : undefined
    });
    
    if (this.pendingTxs.size > this.config.maxPendingTxs) {
      const oldest = this.pendingTxs.keys().next().value;
      if (oldest) this.pendingTxs.delete(oldest);
    }
  }

  broadcastCheckpointSignature(payload: CheckpointSigPayload): void {
    this.receiveCheckpointSignature(payload);
  }

  updateValidatorSet(validators: Map<string, { publicKey: Uint8Array; weight: number; registeredAt: number }>): void {
    this.validatorSet = new Map(validators);
    this.validatorSetMerkle = this.computeValidatorSetMerkle();
  }

  private computeValidatorSetMerkle(): string {
    const sorted = Array.from(this.validatorSet.entries())
      .sort((a, b) => a[0].localeCompare(b[0]));
    const data = JSON.stringify(sorted.map(([addr, v]) => ({
      addr,
      weight: v.weight,
      registeredAt: v.registeredAt
    })));
    let hash = 0;
    for (let i = 0; i < data.length; i++) {
      hash = ((hash << 5) - hash + data.charCodeAt(i)) | 0;
    }
    return hash.toString(16).padStart(8, '0');
  }

  detectConflict(tx1Hash: string, tx2Hash: string, account: string): ConflictRecord {
    const conflictId = [tx1Hash, tx2Hash].sort().join(':');
    
    if (this.conflicts.has(conflictId)) {
      return this.conflicts.get(conflictId)!;
    }
    
    const conflict: ConflictRecord = {
      conflictId,
      txHash1: tx1Hash,
      txHash2: tx2Hash,
      account,
      detectedAt: Date.now(),
      votes: new Map()
    };
    
    this.conflicts.set(conflictId, conflict);
    console.log(`Conflict detected: ${conflictId} for account ${account.slice(0, 16)}...`);
    
    this.voteForConflict(conflict);
    
    return conflict;
  }

  private voteForConflict(conflict: ConflictRecord): void {
    const weight1 = this.getCumulativeWeight(conflict.txHash1);
    const weight2 = this.getCumulativeWeight(conflict.txHash2);
    
    const myWeight = this.getLocalValidatorWeight();
    const votedFor = weight1 >= weight2 ? conflict.txHash1 : conflict.txHash2;
    
    conflict.votes.set(this.nodeId, { voterWeight: myWeight, votedFor });
    
    this.checkConflictResolution(conflict);
  }

  private getCumulativeWeight(txHash: string): number {
    const node = this.consensus.getNode(txHash);
    if (!node) return 0;
    
    let weight = node.weight;
    const descendants = new Set<string>();
    const queue = [...node.children];
    
    while (queue.length > 0) {
      const childHash = queue.shift()!;
      if (descendants.has(childHash)) continue;
      descendants.add(childHash);
      
      const childNode = this.consensus.getNode(childHash);
      if (childNode) {
        weight += childNode.weight;
        queue.push(...childNode.children);
      }
    }
    
    return weight;
  }

  private getLocalValidatorWeight(): number {
    return 1;
  }

  private checkConflictResolution(conflict: ConflictRecord): void {
    if (conflict.resolvedAt) return;
    
    let totalWeight = 0;
    const voteWeights: Map<string, number> = new Map();
    
    for (const [, vote] of conflict.votes) {
      totalWeight += vote.voterWeight;
      const current = voteWeights.get(vote.votedFor) || 0;
      voteWeights.set(vote.votedFor, current + vote.voterWeight);
    }
    
    const totalValidatorWeight = Array.from(this.validatorSet.values())
      .reduce((sum, v) => sum + v.weight, 0) || 1;
    
    for (const [txHash, weight] of voteWeights) {
      if (weight / totalValidatorWeight >= this.config.conflictResolutionThreshold) {
        const loserHash = txHash === conflict.txHash1 ? conflict.txHash2 : conflict.txHash1;
        this.resolveConflict(conflict, txHash, loserHash);
        break;
      }
    }
  }

  private async resolveConflict(conflict: ConflictRecord, winnerHash: string, loserHash: string): Promise<void> {
    conflict.resolvedAt = Date.now();
    conflict.winnerHash = winnerHash;
    this.resolvedConflicts.set(conflict.conflictId, winnerHash);
    
    console.log(`Conflict ${conflict.conflictId} resolved: winner=${winnerHash.slice(0, 16)}...`);
    
    if (this.onConflictResolved) {
      await this.onConflictResolved(conflict.conflictId, winnerHash, loserHash);
    }
  }

  private async gossipRound(): Promise<void> {
    const peers = this.peerSync.getOnlinePeers();
    if (peers.length === 0) return;
    
    const txBatch = Array.from(this.pendingTxs.values()).slice(0, this.config.txBatchSize);
    
    for (const peer of peers) {
      try {
        if (txBatch.length > 0) {
          await this.sendTxBatch(peer, txBatch);
        }
        
        await this.sendTipAnnounce(peer);
        
        await this.sendPendingCheckpointSigs(peer);
        
        await this.sendValidatorSet(peer);
        
        await this.sendConflictResolutions(peer);
      } catch (err) {
      }
    }
    
    for (const payload of txBatch) {
      this.pendingTxs.delete(payload.tx.hash);
    }
  }

  private async sendTxBatch(peer: PeerInfo, txBatch: TxGossipPayload[]): Promise<void> {
    const message: GossipMessage = {
      type: 'tx',
      nodeId: this.nodeId,
      timestamp: Date.now(),
      payload: txBatch
    };
    
    await this.sendGossipMessage(peer.url, message);
  }

  private async sendTipAnnounce(peer: PeerInfo): Promise<void> {
    const payload: TipAnnouncePayload = {
      tips: this.consensus.getTips(),
      tipUrls: this.consensus.getTipUrls(),
      dagSize: this.consensus.getDAGSize(),
      merkleRoot: this.state.getMerkleRoot()
    };
    
    const message: GossipMessage = {
      type: 'tip_announce',
      nodeId: this.nodeId,
      timestamp: Date.now(),
      payload
    };
    
    await this.sendGossipMessage(peer.url, message);
  }

  private async sendPendingCheckpointSigs(peer: PeerInfo): Promise<void> {
    for (const [checkpointId, pending] of this.pendingCheckpointSigs) {
      for (const [validatorAddr, sig] of pending.signatures) {
        const payload: CheckpointSigPayload = {
          checkpointId,
          height: pending.height,
          validatorAddress: validatorAddr,
          signature: sig.signature,
          publicKey: Array.from(sig.publicKey),
          weight: sig.weight
        };
        
        const message: GossipMessage = {
          type: 'checkpoint_sig',
          nodeId: this.nodeId,
          timestamp: Date.now(),
          payload
        };
        
        await this.sendGossipMessage(peer.url, message);
      }
    }
  }

  private async sendValidatorSet(peer: PeerInfo): Promise<void> {
    const validators = Array.from(this.validatorSet.entries()).map(([addr, v]) => ({
      address: addr,
      publicKey: Array.from(v.publicKey),
      weight: v.weight,
      registeredAt: v.registeredAt
    }));
    
    const payload: ValidatorSetPayload = {
      validators,
      merkleRoot: this.validatorSetMerkle,
      timestamp: Date.now()
    };
    
    const message: GossipMessage = {
      type: 'validator_set',
      nodeId: this.nodeId,
      timestamp: Date.now(),
      payload
    };
    
    await this.sendGossipMessage(peer.url, message);
  }

  private async sendConflictResolutions(peer: PeerInfo): Promise<void> {
    for (const [conflictId, conflict] of this.conflicts) {
      if (conflict.resolvedAt && conflict.winnerHash) {
        const payload: ConflictResolutionPayload = {
          conflictId,
          txHash1: conflict.txHash1,
          txHash2: conflict.txHash2,
          winnerHash: conflict.winnerHash,
          cumulativeWeight1: this.getCumulativeWeight(conflict.txHash1),
          cumulativeWeight2: this.getCumulativeWeight(conflict.txHash2),
          resolvedBy: this.nodeId,
          timestamp: conflict.resolvedAt
        };
        
        const message: GossipMessage = {
          type: 'conflict_resolution',
          nodeId: this.nodeId,
          timestamp: Date.now(),
          payload
        };
        
        await this.sendGossipMessage(peer.url, message);
      }
    }
  }

  private async sendGossipMessage(peerUrl: string, message: GossipMessage): Promise<void> {
    try {
      await fetch(`${peerUrl}/api/gossip`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(message),
        signal: AbortSignal.timeout(5000)
      });
    } catch {
    }
  }

  async receiveGossipMessage(message: GossipMessage): Promise<void> {
    if (message.nodeId === this.nodeId) return;
    
    switch (message.type) {
      case 'tx':
        await this.receiveTxBatch(message.payload as TxGossipPayload[]);
        break;
      case 'tip_announce':
        await this.receiveTipAnnounce(message.payload as TipAnnouncePayload, message.nodeId);
        break;
      case 'checkpoint_sig':
        this.receiveCheckpointSignature(message.payload as CheckpointSigPayload);
        break;
      case 'validator_set':
        this.receiveValidatorSet(message.payload as ValidatorSetPayload);
        break;
      case 'conflict_resolution':
        await this.receiveConflictResolution(message.payload as ConflictResolutionPayload);
        break;
    }
  }

  private async receiveTxBatch(txBatch: TxGossipPayload[]): Promise<void> {
    for (const payload of txBatch) {
      if (this.seenTxHashes.has(payload.tx.hash)) continue;
      if (this.consensus.hasTransaction(payload.tx.hash)) {
        this.seenTxHashes.add(payload.tx.hash);
        continue;
      }
      
      this.seenTxHashes.add(payload.tx.hash);
      
      if (this.onTxReceived) {
        const publicKey = payload.publicKey ? new Uint8Array(payload.publicKey) : undefined;
        const accepted = await this.onTxReceived(payload.tx, publicKey);
        
        if (accepted) {
          this.pendingTxs.set(payload.tx.hash, payload);
        }
      }
    }
  }

  private lastTipWarnTime: Map<string, number> = new Map();
  private static readonly TIP_WARN_INTERVAL_MS = 30000;
  private static readonly MAX_HEALTHY_TIPS = 50;
  
  private async receiveTipAnnounce(payload: TipAnnouncePayload, fromNodeId: string): Promise<void> {
    if (payload.tips.length > GossipService.MAX_HEALTHY_TIPS) {
      const lastWarn = this.lastTipWarnTime.get(fromNodeId) || 0;
      if (Date.now() - lastWarn > GossipService.TIP_WARN_INTERVAL_MS) {
        console.warn(`Ignoring unhealthy peer ${fromNodeId.slice(0, 8)} with ${payload.tips.length} tips (max: ${GossipService.MAX_HEALTHY_TIPS})`);
        this.lastTipWarnTime.set(fromNodeId, Date.now());
      }
      return;
    }
    
    const ourTips = new Set(this.consensus.getTips());
    const missingTips = payload.tips.filter(tip => !ourTips.has(tip) && !this.consensus.hasTransaction(tip));
    
    if (missingTips.length > 0) {
      const lastWarn = this.lastTipWarnTime.get(fromNodeId) || 0;
      if (Date.now() - lastWarn > GossipService.TIP_WARN_INTERVAL_MS) {
        console.log(`Node ${fromNodeId.slice(0, 8)} has ${missingTips.length} tips we're missing`);
        this.lastTipWarnTime.set(fromNodeId, Date.now());
      }
    }
  }

  private receiveCheckpointSignature(payload: CheckpointSigPayload): void {
    let pending = this.pendingCheckpointSigs.get(payload.checkpointId);
    
    if (!pending) {
      const totalWeight = Array.from(this.validatorSet.values())
        .reduce((sum, v) => sum + v.weight, 0) || 1;
      
      pending = {
        checkpointId: payload.checkpointId,
        height: payload.height,
        signatures: new Map(),
        totalWeight: 0,
        requiredWeight: totalWeight * (this.config.checkpointQuorumPercent / 100)
      };
      this.pendingCheckpointSigs.set(payload.checkpointId, pending);
    }
    
    if (!pending.signatures.has(payload.validatorAddress)) {
      pending.signatures.set(payload.validatorAddress, {
        signature: payload.signature,
        publicKey: new Uint8Array(payload.publicKey),
        weight: payload.weight,
        receivedAt: Date.now()
      });
      pending.totalWeight += payload.weight;
      
      console.log(`Received checkpoint signature from ${payload.validatorAddress.slice(0, 16)}... (${pending.totalWeight}/${pending.requiredWeight} weight)`);
      
      if (pending.totalWeight >= pending.requiredWeight && this.onCheckpointQuorum) {
        const sigMap = new Map<string, { signature: string; publicKey: Uint8Array; weight: number }>();
        for (const [addr, sig] of pending.signatures) {
          sigMap.set(addr, { signature: sig.signature, publicKey: sig.publicKey, weight: sig.weight });
        }
        this.onCheckpointQuorum(payload.checkpointId, sigMap);
      }
    }
  }

  private receiveValidatorSet(payload: ValidatorSetPayload): void {
    if (payload.merkleRoot !== this.validatorSetMerkle) {
      console.log(`Validator set mismatch: local=${this.validatorSetMerkle}, remote=${payload.merkleRoot}`);
    }
  }

  private async receiveConflictResolution(payload: ConflictResolutionPayload): Promise<void> {
    if (this.resolvedConflicts.has(payload.conflictId)) {
      const existingWinner = this.resolvedConflicts.get(payload.conflictId);
      if (existingWinner !== payload.winnerHash) {
        console.warn(`Conflict resolution mismatch for ${payload.conflictId}`);
      }
      return;
    }
    
    this.resolvedConflicts.set(payload.conflictId, payload.winnerHash);
    
    if (this.onConflictResolved) {
      const loserHash = payload.winnerHash === payload.txHash1 ? payload.txHash2 : payload.txHash1;
      await this.onConflictResolved(payload.conflictId, payload.winnerHash, loserHash);
    }
  }

  isConflictResolved(txHash1: string, txHash2: string): boolean {
    const conflictId = [txHash1, txHash2].sort().join(':');
    return this.resolvedConflicts.has(conflictId);
  }

  getConflictWinner(txHash1: string, txHash2: string): string | undefined {
    const conflictId = [txHash1, txHash2].sort().join(':');
    return this.resolvedConflicts.get(conflictId);
  }

  getStats(): {
    pendingTxs: number;
    seenTxs: number;
    pendingCheckpoints: number;
    activeConflicts: number;
    resolvedConflicts: number;
    validatorCount: number;
  } {
    return {
      pendingTxs: this.pendingTxs.size,
      seenTxs: this.seenTxHashes.size,
      pendingCheckpoints: this.pendingCheckpointSigs.size,
      activeConflicts: Array.from(this.conflicts.values()).filter(c => !c.resolvedAt).length,
      resolvedConflicts: this.resolvedConflicts.size,
      validatorCount: this.validatorSet.size
    };
  }

  toJSON(): object {
    return {
      seenTxHashes: Array.from(this.seenTxHashes).slice(-1000),
      pendingCheckpointSigs: Array.from(this.pendingCheckpointSigs.entries()).map(([id, pending]) => ({
        checkpointId: id,
        height: pending.height,
        signatures: Array.from(pending.signatures.entries()).map(([addr, sig]) => ({
          validatorAddress: addr,
          signature: sig.signature,
          publicKey: Array.from(sig.publicKey),
          weight: sig.weight,
          receivedAt: sig.receivedAt
        })),
        totalWeight: pending.totalWeight,
        requiredWeight: pending.requiredWeight
      })),
      resolvedConflicts: Array.from(this.resolvedConflicts.entries()),
      validatorSet: Array.from(this.validatorSet.entries()).map(([addr, v]) => ({
        address: addr,
        publicKey: Array.from(v.publicKey),
        weight: v.weight,
        registeredAt: v.registeredAt
      })),
      validatorSetMerkle: this.validatorSetMerkle
    };
  }

  static fromJSON(
    data: any,
    consensus: Consensus,
    state: StateManager,
    peerSync: PeerSyncService,
    nodeId: string,
    config?: Partial<GossipConfig>
  ): GossipService {
    const service = new GossipService(consensus, state, peerSync, nodeId, config);
    
    if (data.seenTxHashes) {
      service.seenTxHashes = new Set(data.seenTxHashes);
    }
    
    if (data.pendingCheckpointSigs) {
      for (const pending of data.pendingCheckpointSigs) {
        const sigMap = new Map<string, { signature: string; publicKey: Uint8Array; weight: number; receivedAt: number }>();
        for (const sig of pending.signatures) {
          sigMap.set(sig.validatorAddress, {
            signature: sig.signature,
            publicKey: new Uint8Array(sig.publicKey),
            weight: sig.weight,
            receivedAt: sig.receivedAt
          });
        }
        service.pendingCheckpointSigs.set(pending.checkpointId, {
          checkpointId: pending.checkpointId,
          height: pending.height,
          signatures: sigMap,
          totalWeight: pending.totalWeight,
          requiredWeight: pending.requiredWeight
        });
      }
    }
    
    if (data.resolvedConflicts) {
      service.resolvedConflicts = new Map(data.resolvedConflicts);
    }
    
    if (data.validatorSet) {
      for (const v of data.validatorSet) {
        service.validatorSet.set(v.address, {
          publicKey: new Uint8Array(v.publicKey),
          weight: v.weight,
          registeredAt: v.registeredAt
        });
      }
      service.validatorSetMerkle = data.validatorSetMerkle || service.computeValidatorSetMerkle();
    }
    
    return service;
  }
}
