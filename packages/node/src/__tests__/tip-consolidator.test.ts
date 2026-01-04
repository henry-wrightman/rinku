import { describe, it, expect, beforeEach, vi } from 'vitest';
import { TipConsolidatorService, type TipConsolidatorConfig } from '../tip-consolidator.js';
import { StateManager } from '../state.js';
import type { SignedTransaction } from '@rinku/core';

const { mockTipConsolidationInc, mockTipsConsolidatedInc } = vi.hoisted(() => ({
  mockTipConsolidationInc: vi.fn(),
  mockTipsConsolidatedInc: vi.fn(),
}));

vi.mock('@rinku/core', async () => {
  const actual = await vi.importActual('@rinku/core');
  return {
    ...actual,
    hashTransaction: vi.fn().mockResolvedValue('mockhash123'),
    sign: vi.fn().mockResolvedValue('mocksig456'),
  };
});

vi.mock('../telemetry.js', () => ({
  tipConsolidationCounter: { inc: mockTipConsolidationInc },
  tipsConsolidatedCounter: { inc: mockTipsConsolidatedInc },
}));

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
    mockTipConsolidationInc.mockClear();
    mockTipsConsolidatedInc.mockClear();
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

  describe('Account Auto-Creation', () => {
    it('should create validator account if missing', async () => {
      const mockKey = {
        fingerprint: 'newvalidator',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      
      expect(state.getAccount('newvalidator')).toBeUndefined();
      
      mockConsensus.setTipCount(250);
      
      await (service as any).createConsolidationTx();
      
      expect(state.getAccount('newvalidator')).toBeDefined();
      expect(state.getAccount('newvalidator')!.nonce).toBe(1);
    });
  });

  describe('Cooldown Behavior', () => {
    it('should block consolidation during cooldown', async () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      state.createAccount('validator123', 0);
      
      const recentCooldownTime = Date.now();
      (service as any).lastConsolidationAt = recentCooldownTime;
      
      mockConsensus.setTipCount(250);
      
      await (service as any).checkAndConsolidate().catch(() => {});
      
      expect((service as any).lastConsolidationAt).toBe(recentCooldownTime);
      expect(mockConsensus.addedTransactions.length).toBe(0);
    });

    it('should consolidate after cooldown elapses', async () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      state.createAccount('validator123', 0);
      
      const oldCooldownTime = Date.now() - testConfig.cooldownMs - 100;
      (service as any).lastConsolidationAt = oldCooldownTime;
      
      mockConsensus.setTipCount(250);
      
      await (service as any).checkAndConsolidate();
      
      expect((service as any).lastConsolidationAt).toBeGreaterThan(oldCooldownTime);
      expect(mockConsensus.addedTransactions.length).toBe(1);
      expect(state.getAccount('validator123')!.nonce).toBe(1);
    });
  });

  describe('Successful Consolidation', () => {
    it('should create consolidation tx when tips exceed threshold', async () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      state.createAccount('validator123', 0);
      
      mockConsensus.setTipCount(250);
      
      await (service as any).checkAndConsolidate();
      
      expect(mockConsensus.addedTransactions.length).toBe(1);
      const tx = mockConsensus.addedTransactions[0];
      expect(tx.kind).toBe('consolidation');
      expect(tx.from).toBe('validator123');
      expect(tx.to).toBe('validator123');
      expect(tx.amount).toBe(0);
      expect(tx.fee).toBe(0);
    });

    it('should update nonce after consolidation', async () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      state.createAccount('validator123', 0);
      
      expect(state.getAccount('validator123')!.nonce).toBe(0);
      
      mockConsensus.setTipCount(250);
      await (service as any).checkAndConsolidate();
      
      expect(state.getAccount('validator123')!.nonce).toBe(1);
    });

    it('should increment stats counters after consolidation', async () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      state.createAccount('validator123', 0);
      
      mockConsensus.setTipCount(250);
      await (service as any).checkAndConsolidate();
      
      const stats = service.getStats();
      expect(stats.totalConsolidations).toBe(1);
      expect(stats.tipsConsolidated).toBe(32);
      expect(stats.lastConsolidationAt).not.toBeNull();
    });

    it('should increment Prometheus counters after consolidation', async () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      state.createAccount('validator123', 0);
      
      mockConsensus.setTipCount(250);
      await (service as any).checkAndConsolidate();
      
      expect(mockTipConsolidationInc).toHaveBeenCalledTimes(1);
      expect(mockTipsConsolidatedInc).toHaveBeenCalledWith(32);
    });

    it('should reference correct number of tips', async () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      state.createAccount('validator123', 0);
      
      mockConsensus.setTipCount(250);
      await (service as any).checkAndConsolidate();
      
      const tx = mockConsensus.addedTransactions[0];
      expect(tx.tipUrls.length).toBe(testConfig.tipsPerConsolidation);
    });

    it('should handle sequential consolidations with correct nonces', async () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      state.createAccount('validator123', 0);
      
      mockConsensus.setTipCount(250);
      
      await (service as any).createConsolidationTx();
      expect(mockConsensus.addedTransactions[0].nonce).toBe(1);
      expect(state.getAccount('validator123')!.nonce).toBe(1);
      
      (service as any).lastConsolidationAt = 0;
      
      await (service as any).createConsolidationTx();
      expect(mockConsensus.addedTransactions[1].nonce).toBe(2);
      expect(state.getAccount('validator123')!.nonce).toBe(2);
    });
  });

  describe('Error Handling', () => {
    it('should not update stats when consensus rejects transaction', async () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      state.createAccount('validator123', 0);
      
      mockConsensus.setTipCount(250);
      
      const initialLastConsolidation = (service as any).lastConsolidationAt;
      
      const originalAddTransaction = mockConsensus.addTransaction;
      const addTransactionMock = vi.fn().mockRejectedValue(new Error('Consensus rejection'));
      mockConsensus.addTransaction = addTransactionMock;
      
      await (service as any).createConsolidationTx();
      
      expect(addTransactionMock).toHaveBeenCalledTimes(1);
      
      const stats = service.getStats();
      expect(stats.totalConsolidations).toBe(0);
      expect(stats.tipsConsolidated).toBe(0);
      expect(mockTipConsolidationInc).not.toHaveBeenCalled();
      expect(mockTipsConsolidatedInc).not.toHaveBeenCalled();
      expect((service as any).lastConsolidationAt).toBe(initialLastConsolidation);
      
      mockConsensus.addTransaction = originalAddTransaction;
    });

    it('should not update stats when state rejects transaction', async () => {
      const mockKey = {
        fingerprint: 'validator123',
        publicKey: {} as any,
        privateKey: {} as any,
      };
      service.setValidatorKey(mockKey);
      state.createAccount('validator123', 0);
      
      mockConsensus.setTipCount(250);
      
      const initialLastConsolidation = (service as any).lastConsolidationAt;
      const initialTxCount = mockConsensus.addedTransactions.length;
      
      const originalApplyTransaction = state.applyTransaction.bind(state);
      const applyTransactionMock = vi.fn().mockResolvedValue(false);
      state.applyTransaction = applyTransactionMock;
      
      const originalAddTransaction = mockConsensus.addTransaction;
      const addTransactionMock = vi.fn();
      mockConsensus.addTransaction = addTransactionMock;
      
      await (service as any).createConsolidationTx();
      
      expect(applyTransactionMock).toHaveBeenCalledTimes(1);
      expect(addTransactionMock).not.toHaveBeenCalled();
      
      const stats = service.getStats();
      expect(stats.totalConsolidations).toBe(0);
      expect(stats.tipsConsolidated).toBe(0);
      expect(mockTipConsolidationInc).not.toHaveBeenCalled();
      expect(mockTipsConsolidatedInc).not.toHaveBeenCalled();
      expect((service as any).lastConsolidationAt).toBe(initialLastConsolidation);
      
      state.applyTransaction = originalApplyTransaction;
      mockConsensus.addTransaction = originalAddTransaction;
    });
  });
});
