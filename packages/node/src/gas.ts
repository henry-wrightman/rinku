import type { GasPrice, GasConfig, FeeStats } from '@rinku/core';

/**
 * EIP-1559 style gas pricing
 * - Price adjusts based on utilization vs target, not paid fees
 * - Prevents runaway feedback loops
 * - Smooth adjustments capped at ~12.5% per period
 */
export class GasService {
  private totalBurned = 0;
  private totalToValidators = 0;
  private txCount = 0;
  
  // EIP-1559 style tracking
  private baseFee: number;
  private txsThisPeriod = 0;
  private periodStartTime = Date.now();
  private readonly PERIOD_MS = 15000; // 15s checkpoint interval
  private readonly TARGET_TXS_PER_PERIOD = 15; // Target ~1 TPS
  private readonly MAX_CHANGE_PERCENT = 0.125; // 12.5% max change per period
  private readonly ELASTICITY = 2; // How much over target before max increase
  
  private config: GasConfig = {
    minFee: 0.001,
    maxFee: 10, // Reduced from 100 - more reasonable cap
    baseFee: 0.01,
    feeMultiplier: 1.0,
    burnPercent: 50,
    validatorPercent: 50
  };

  constructor(config?: Partial<GasConfig>) {
    if (config) {
      this.config = { ...this.config, ...config };
    }
    this.baseFee = this.config.baseFee;
  }

  getConfig(): GasConfig {
    return { ...this.config };
  }

  setConfig(config: Partial<GasConfig>): void {
    this.config = { ...this.config, ...config };
  }

  /**
   * EIP-1559 style price adjustment
   * - If utilization > target: increase baseFee
   * - If utilization < target: decrease baseFee
   * - Change capped at 12.5% per period
   */
  private adjustBaseFee(): void {
    const now = Date.now();
    if (now - this.periodStartTime < this.PERIOD_MS) {
      return; // Still in current period
    }

    // Calculate utilization ratio
    const utilization = this.txsThisPeriod / this.TARGET_TXS_PER_PERIOD;
    
    // Calculate change factor (-12.5% to +12.5%)
    // At 0 txs: -12.5%, at target: 0%, at 2x target: +12.5%
    const changeRatio = Math.max(-1, Math.min(1, (utilization - 1) / (this.ELASTICITY - 1)));
    const changeFactor = 1 + (changeRatio * this.MAX_CHANGE_PERCENT);
    
    // Apply change with bounds
    this.baseFee = Math.max(
      this.config.minFee,
      Math.min(this.config.maxFee, this.baseFee * changeFactor)
    );
    
    // Reset period
    this.txsThisPeriod = 0;
    this.periodStartTime = now;
  }

  getCurrentGasPrice(): GasPrice {
    this.adjustBaseFee();
    
    const currentPrice = Math.max(
      this.config.minFee,
      Math.min(this.config.maxFee, this.baseFee * this.config.feeMultiplier)
    );

    return {
      current: Math.round(currentPrice * 1000000) / 1000000,
      min: this.config.minFee,
      max: this.config.maxFee,
      avgLast100: Math.round(this.baseFee * 1000000) / 1000000, // Now shows baseFee
      lastUpdated: Date.now()
    };
  }

  recordFee(fee: number): { burned: number; toValidators: number } {
    if (fee <= 0) {
      return { burned: 0, toValidators: 0 };
    }

    // Track for EIP-1559 adjustment
    this.txsThisPeriod++;

    const burned = fee * (this.config.burnPercent / 100);
    const toValidators = fee * (this.config.validatorPercent / 100);

    this.totalBurned += burned;
    this.totalToValidators += toValidators;
    this.txCount++;

    return { burned, toValidators };
  }

  getStats(): FeeStats {
    return {
      totalBurned: Math.round(this.totalBurned * 1000000) / 1000000,
      totalToValidators: Math.round(this.totalToValidators * 1000000) / 1000000,
      avgFee: Math.round(this.baseFee * 1000000) / 1000000,
      txCount: this.txCount
    };
  }

  validateFee(fee: number): { valid: boolean; error?: string } {
    if (fee < 0) {
      return { valid: false, error: 'Fee cannot be negative' };
    }
    if (fee < this.config.minFee && fee !== 0) {
      return { valid: false, error: `Fee ${fee} below minimum ${this.config.minFee}` };
    }
    if (fee > this.config.maxFee) {
      return { valid: false, error: `Fee ${fee} exceeds maximum ${this.config.maxFee}` };
    }
    return { valid: true };
  }

  toJSON(): object {
    return {
      config: this.config,
      baseFee: this.baseFee,
      txsThisPeriod: this.txsThisPeriod,
      periodStartTime: this.periodStartTime,
      totalBurned: this.totalBurned,
      totalToValidators: this.totalToValidators,
      txCount: this.txCount
    };
  }

  static fromJSON(data: any): GasService {
    const service = new GasService(data.config);
    service.baseFee = data.baseFee ?? data.config?.baseFee ?? 0.01;
    service.txsThisPeriod = data.txsThisPeriod || 0;
    service.periodStartTime = data.periodStartTime || Date.now();
    service.totalBurned = data.totalBurned || 0;
    service.totalToValidators = data.totalToValidators || 0;
    service.txCount = data.txCount || 0;
    return service;
  }
}
