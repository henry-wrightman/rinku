import {
  type SignedTransaction,
  type KeyPair,
  hashTransaction,
  sign,
} from '@rinku/core';
import type { Consensus } from './consensus.js';
import type { StateManager } from './state.js';

export interface TipConsolidatorConfig {
  upperThreshold: number;
  lowerThreshold: number;
  tipsPerConsolidation: number;
  intervalMs: number;
  cooldownMs: number;
}

const DEFAULT_CONFIG: TipConsolidatorConfig = {
  upperThreshold: 200,
  lowerThreshold: 100,
  tipsPerConsolidation: 32,
  intervalMs: 3000,
  cooldownMs: 5000,
};

export interface ConsolidationStats {
  totalConsolidations: number;
  tipsConsolidated: number;
  lastConsolidationAt: number | null;
  currentTipCount: number;
  isActive: boolean;
}

export class TipConsolidatorService {
  private consensus: Consensus;
  private state: StateManager;
  private validatorKey: KeyPair | null = null;
  private config: TipConsolidatorConfig;
  private intervalHandle: ReturnType<typeof setInterval> | null = null;
  private lastConsolidationAt: number = 0;
  private totalConsolidations: number = 0;
  private tipsConsolidated: number = 0;
  private isRunning: boolean = false;

  constructor(
    consensus: Consensus,
    state: StateManager,
    config: Partial<TipConsolidatorConfig> = {}
  ) {
    this.consensus = consensus;
    this.state = state;
    this.config = { ...DEFAULT_CONFIG, ...config };
  }

  setValidatorKey(key: KeyPair): void {
    this.validatorKey = key;
    console.log(`[TipConsolidator] Validator key set: ${key.fingerprint.slice(0, 16)}...`);
  }

  start(): void {
    if (this.intervalHandle) {
      return;
    }

    this.isRunning = true;
    this.intervalHandle = setInterval(() => {
      this.checkAndConsolidate().catch(err => {
        console.error('[TipConsolidator] Error:', err);
      });
    }, this.config.intervalMs);

    console.log(`[TipConsolidator] Started (threshold: ${this.config.upperThreshold}, interval: ${this.config.intervalMs}ms)`);
  }

  stop(): void {
    if (this.intervalHandle) {
      clearInterval(this.intervalHandle);
      this.intervalHandle = null;
    }
    this.isRunning = false;
    console.log('[TipConsolidator] Stopped');
  }

  private async checkAndConsolidate(): Promise<void> {
    if (!this.validatorKey) {
      return;
    }

    const now = Date.now();
    if (now - this.lastConsolidationAt < this.config.cooldownMs) {
      return;
    }

    const tipCount = this.consensus.getTips().length;
    
    if (tipCount <= this.config.lowerThreshold) {
      return;
    }

    if (tipCount > this.config.upperThreshold) {
      await this.createConsolidationTx();
    }
  }

  private async createConsolidationTx(): Promise<void> {
    if (!this.validatorKey) {
      return;
    }

    const tipUrls = this.consensus.getTipUrls();
    const numTips = Math.min(this.config.tipsPerConsolidation, tipUrls.length);
    
    if (numTips < 2) {
      return;
    }

    const oldestTipUrls = tipUrls.slice(0, numTips);

    const sender = this.state.getAccount(this.validatorKey.fingerprint);
    const nonce = sender ? sender.nonce + 1 : 1;

    const tx: Omit<SignedTransaction, 'hash'> = {
      from: this.validatorKey.fingerprint,
      to: this.validatorKey.fingerprint,
      amount: 0,
      fee: 0,
      nonce,
      tipUrls: oldestTipUrls,
      ts: Date.now(),
      kind: 'consolidation',
      sig: '',
    };

    const hash = await hashTransaction(tx);
    const sig = await sign(hash, this.validatorKey.privateKey);

    const signedTx: SignedTransaction = {
      ...tx,
      hash,
      sig,
    };

    try {
      await this.consensus.addTransaction(signedTx);
      
      await this.state.applyTransaction(signedTx, { skipChecks: true });
      
      this.lastConsolidationAt = Date.now();
      this.totalConsolidations++;
      this.tipsConsolidated += numTips;
      
      console.log(`[TipConsolidator] Created consolidation tx referencing ${numTips} tips (total: ${this.totalConsolidations}, tips now: ${this.consensus.getTips().length})`);
    } catch (err) {
      console.error('[TipConsolidator] Failed to add consolidation tx:', err);
    }
  }

  getStats(): ConsolidationStats {
    return {
      totalConsolidations: this.totalConsolidations,
      tipsConsolidated: this.tipsConsolidated,
      lastConsolidationAt: this.lastConsolidationAt || null,
      currentTipCount: this.consensus.getTips().length,
      isActive: this.isRunning && this.validatorKey !== null,
    };
  }

  getConfig(): TipConsolidatorConfig {
    return { ...this.config };
  }
}
