import type { GasPrice, GasConfig, FeeStats } from '@rinku/core';

export class GasService {
  private recentFees: number[] = [];
  private readonly MAX_RECENT_FEES = 100;
  private totalBurned = 0;
  private totalToValidators = 0;
  private txCount = 0;
  
  private config: GasConfig = {
    minFee: 0.001,
    maxFee: 100,
    baseFee: 0.01,
    feeMultiplier: 1.0,
    burnPercent: 50,
    validatorPercent: 50
  };

  constructor(config?: Partial<GasConfig>) {
    if (config) {
      this.config = { ...this.config, ...config };
    }
  }

  getConfig(): GasConfig {
    return { ...this.config };
  }

  setConfig(config: Partial<GasConfig>): void {
    this.config = { ...this.config, ...config };
  }

  getCurrentGasPrice(): GasPrice {
    const avgFee = this.recentFees.length > 0
      ? this.recentFees.reduce((a, b) => a + b, 0) / this.recentFees.length
      : this.config.baseFee;

    const demandMultiplier = Math.min(2, 1 + (this.recentFees.length / this.MAX_RECENT_FEES));
    const currentPrice = Math.max(
      this.config.minFee,
      Math.min(this.config.maxFee, avgFee * demandMultiplier * this.config.feeMultiplier)
    );

    return {
      current: Math.round(currentPrice * 1000000) / 1000000,
      min: this.config.minFee,
      max: this.config.maxFee,
      avgLast100: Math.round(avgFee * 1000000) / 1000000,
      lastUpdated: Date.now()
    };
  }

  recordFee(fee: number): { burned: number; toValidators: number } {
    if (fee <= 0) {
      return { burned: 0, toValidators: 0 };
    }

    this.recentFees.push(fee);
    if (this.recentFees.length > this.MAX_RECENT_FEES) {
      this.recentFees.shift();
    }

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
      avgFee: this.recentFees.length > 0
        ? Math.round((this.recentFees.reduce((a, b) => a + b, 0) / this.recentFees.length) * 1000000) / 1000000
        : 0,
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
      recentFees: this.recentFees,
      totalBurned: this.totalBurned,
      totalToValidators: this.totalToValidators,
      txCount: this.txCount
    };
  }

  static fromJSON(data: any): GasService {
    const service = new GasService(data.config);
    service.recentFees = data.recentFees || [];
    service.totalBurned = data.totalBurned || 0;
    service.totalToValidators = data.totalToValidators || 0;
    service.txCount = data.txCount || 0;
    return service;
  }
}
