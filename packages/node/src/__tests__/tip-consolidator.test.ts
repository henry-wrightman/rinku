import { describe, it, expect, beforeEach, vi } from 'vitest';
import { TipConsolidatorService, type TipConsolidatorConfig } from '../tip-consolidator.js';
import { StateManager } from '../state.js';
import type { SignedTransaction } from '@rinku/core';

const mockConsensus = {
  tips: [] as string[],
  tipUrls: [] as string[],
  addedTransactions: [] as SignedTransaction[],
  
  getTips() {
    return this.tips;
  },
  
  getTipUrls() {
    return this.tipUrls;
  },
  
  async addTransaction(tx: SignedTransaction) {
    this.addedTransactions.push(tx);
  },
  
  reset() {
    this.tips = [];
    this.tipUrls = [];
    this.addedTransactions = [];
  },
  
  setTipCount(count: number) {
    this.tips = Array(count).fill('tip');
    this.tipUrls = Array(count).fill(0).map((_, i) => `rinku://tx/tip${i}`);
  },
};

vi.mock('../telemetry.js', () => ({
  tipConsolidationCounter: { inc: vi.fn() },
  tipsConsolidatedCounter: { inc: vi.fn() },
}));

describe('TipConsolidatorService', () => {
  let service: TipConsolidatorService;
  let state: StateManager;
  const testConfig: TipConsolidatorConfig = {
    upperThreshold: 200,
    lowerThreshold: 100,
    tipsPerConsolidation: 32,
    intervalMs: 100,
    cooldownMs: 50,
  };

  beforeEach(() => {
    mockConsensus.reset();
    state = new StateManager();
    service = new TipConsolidatorService(
      mockConsensus as any,
      state,
      testConfig
    );
  });

  describe('Configuration', () => {
    it('should use provided config', () => {
      const config = service.getConfig();
      expect(config.upperThreshold).toBe(200);
      expect(config.lowerThreshold).toBe(100);
      expect(config.tipsPerConsolidation).toBe(32);
    });

    it('should merge partial config with defaults', () => {
      const partialService = new TipConsolidatorService(
        mockConsensus as any,
        state,
        { upperThreshold: 300 }
      );
      const config = partialService.getConfig();
      expect(config.upperThreshold).toBe(300);
      expect(config.lowerThreshold).toBe(100);
    });
  });

  describe('Stats', () => {
    it('should report initial stats', () => {
      const stats = service.getStats();
      expect(stats.totalConsolidations).toBe(0);
      expect(stats.tipsConsolidated).toBe(0);
      expect(stats.lastConsolidationAt).toBeNull();
      expect(stats.isActive).toBe(false);
    });

    it('should report inactive without validator key', () => {
      service.start();
      const stats = service.getStats();
      expect(stats.isActive).toBe(false);
    });

    it('should report current tip count', () => {
      mockConsensus.setTipCount(150);
      const stats = service.getStats();
      expect(stats.currentTipCount).toBe(150);
    });
  });

  describe('Threshold Logic', () => {
    it('should not consolidate below lower threshold', async () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      state.createAccount('validator123', 0);
      
      mockConsensus.setTipCount(50);
      
      await (service as any).checkAndConsolidate();
      
      expect(mockConsensus.addedTransactions.length).toBe(0);
    });

    it('should not consolidate between thresholds', async () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      state.createAccount('validator123', 0);
      
      mockConsensus.setTipCount(150);
      
      await (service as any).checkAndConsolidate();
      
      expect(mockConsensus.addedTransactions.length).toBe(0);
    });
  });

  describe('Validator Key Management', () => {
    it('should not consolidate without validator key', async () => {
      mockConsensus.setTipCount(300);
      
      await (service as any).checkAndConsolidate();
      
      expect(mockConsensus.addedTransactions.length).toBe(0);
    });

    it('should set validator key', () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      
      service.setValidatorKey(mockKey);
      
      expect((service as any).validatorKey).toBe(mockKey);
    });
  });

  describe('Cooldown', () => {
    it('should respect cooldown period', async () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      state.createAccount('validator123', 0);
      
      (service as any).lastConsolidationAt = Date.now();
      
      mockConsensus.setTipCount(300);
      
      await (service as any).checkAndConsolidate();
      
      expect(mockConsensus.addedTransactions.length).toBe(0);
    });
  });

  describe('Service Lifecycle', () => {
    it('should start and stop', () => {
      service.start();
      expect((service as any).isRunning).toBe(true);
      expect((service as any).intervalHandle).not.toBeNull();
      
      service.stop();
      expect((service as any).isRunning).toBe(false);
      expect((service as any).intervalHandle).toBeNull();
    });

    it('should not start twice', () => {
      service.start();
      const firstHandle = (service as any).intervalHandle;
      
      service.start();
      const secondHandle = (service as any).intervalHandle;
      
      expect(firstHandle).toBe(secondHandle);
      
      service.stop();
    });
  });
});
