import type { StakePosition } from '@rinku/core';

export const TOKENOMICS_CONFIG = {
  MAX_SUPPLY: 30_000_000,
  GENESIS_ALLOCATION: 6_000_000,
  INITIAL_CHECKPOINT_REWARD: 3.934,
  HALVING_INTERVAL: 3_150_000,
  MIN_CHECKPOINT_REWARD: 0.123,
  HALVINGS_COUNT: 5,
  UNBONDING_PERIOD_MS: 14 * 24 * 60 * 60 * 1000,
  SLASH_DOUBLE_SIGN_PERCENT: 0.15,
  SLASH_INVALID_CHECKPOINT_PERCENT: 0.25,
  SLASH_INVALID_PROOF_PERCENT: 0.20,
  SLASH_INVALID_WITNESS_PERCENT: 0.15,
  SLASH_RECEIPT_TAMPERING_PERCENT: 0.25,
  SLASH_LIVENESS_PERCENT: 0.05,
  SLASH_LIVENESS_REPEAT_PERCENT: 0.10,
  LIVENESS_MISS_THRESHOLD: 3,
  LIVENESS_REPEAT_WINDOW_MS: 30 * 24 * 60 * 60 * 1000,
  STAKE_WEIGHT_PERCENT: 0.70,
  AGE_WEIGHT_PERCENT: 0.30,
  VALIDATOR_FEE_FLOOR_PERCENT: 0.70,
  BURN_CEILING_PERCENT: 0.30,
  SUPPLY_TARGET_FOR_FULL_BURN: 0.50,
  MIN_BOND_FOR_AGE_WEIGHT: 100,
  AGE_DECAY_PER_MISS: 0.10,
};

export type SlashReason = 
  | 'double_sign'
  | 'invalid_checkpoint'
  | 'invalid_proof'
  | 'invalid_witness'
  | 'receipt_tampering'
  | 'liveness_failure'
  | 'liveness_repeat';

export interface SlashEvent {
  id: string;
  validator: string;
  reason: SlashReason;
  amount: number;
  percentSlashed: number;
  checkpointHeight: number;
  timestamp: number;
  details?: string;
}

export interface UnbondingEntry {
  validator: string;
  amount: number;
  startedAt: number;
  availableAt: number;
  slashable: boolean;
}

export interface EmissionStats {
  currentReward: number;
  halvingEpoch: number;
  nextHalvingAt: number;
  totalEmitted: number;
  remainingToEmit: number;
  circulatingSupply: number;
  totalBurned: number;
  validatorFeePercent: number;
  burnPercent: number;
}

export interface FeeSplit {
  validatorShare: number;
  burnShare: number;
}

export interface TokenomicsSnapshot {
  totalEmitted: number;
  totalBurned: number;
  slashEvents: SlashEvent[];
  unbondingQueue: UnbondingEntry[];
  livenessFailures: [string, { count: number; lastFailure: number }][];
}

export class EmissionService {
  private totalEmitted = 0;
  private totalBurned = 0;

  constructor(initialEmitted = 0, initialBurned = 0) {
    this.totalEmitted = initialEmitted;
    this.totalBurned = initialBurned;
  }

  getCheckpointReward(checkpointHeight: number): number {
    const halvings = Math.floor(checkpointHeight / TOKENOMICS_CONFIG.HALVING_INTERVAL);
    const effectiveHalvings = Math.min(halvings, TOKENOMICS_CONFIG.HALVINGS_COUNT);
    const reward = TOKENOMICS_CONFIG.INITIAL_CHECKPOINT_REWARD / Math.pow(2, effectiveHalvings);
    return Math.max(reward, TOKENOMICS_CONFIG.MIN_CHECKPOINT_REWARD);
  }

  getHalvingEpoch(checkpointHeight: number): number {
    return Math.floor(checkpointHeight / TOKENOMICS_CONFIG.HALVING_INTERVAL);
  }

  getNextHalvingHeight(checkpointHeight: number): number {
    const currentEpoch = this.getHalvingEpoch(checkpointHeight);
    return (currentEpoch + 1) * TOKENOMICS_CONFIG.HALVING_INTERVAL;
  }

  recordEmission(amount: number): void {
    this.totalEmitted += amount;
  }

  recordBurn(amount: number): void {
    this.totalBurned += amount;
  }

  getCirculatingSupply(): number {
    return TOKENOMICS_CONFIG.GENESIS_ALLOCATION + this.totalEmitted - this.totalBurned;
  }

  getRemainingToEmit(): number {
    const maxEmittable = TOKENOMICS_CONFIG.MAX_SUPPLY - TOKENOMICS_CONFIG.GENESIS_ALLOCATION;
    return Math.max(0, maxEmittable - this.totalEmitted);
  }

  /**
   * Calculate progressive burn percentage based on circulating supply.
   * Formula: burnPercent = (circulatingSupply / MAX_SUPPLY) / SUPPLY_TARGET * BURN_CEILING
   * This is a LINEAR function where:
   * - At genesis (20% supply): ~12% burn
   * - At 50% supply target: 30% burn (ceiling)
   * - Beyond 50%: capped at 30% burn
   * 
   * circulatingSupply = GENESIS_ALLOCATION + totalEmitted - totalBurned
   * (includes treasury, excludes burned tokens)
   */
  progressiveBurn(): number {
    const circulatingSupply = this.getCirculatingSupply();
    const supplyRatio = circulatingSupply / TOKENOMICS_CONFIG.MAX_SUPPLY;
    
    if (supplyRatio >= TOKENOMICS_CONFIG.SUPPLY_TARGET_FOR_FULL_BURN) {
      return TOKENOMICS_CONFIG.BURN_CEILING_PERCENT;
    }
    
    const burnProgress = supplyRatio / TOKENOMICS_CONFIG.SUPPLY_TARGET_FOR_FULL_BURN;
    return burnProgress * TOKENOMICS_CONFIG.BURN_CEILING_PERCENT;
  }

  getAdaptiveFeeSplit(): FeeSplit {
    const burnPercent = this.progressiveBurn();
    const validatorPercent = Math.max(
      TOKENOMICS_CONFIG.VALIDATOR_FEE_FLOOR_PERCENT,
      1 - burnPercent
    );
    
    return {
      validatorShare: validatorPercent,
      burnShare: 1 - validatorPercent
    };
  }

  calculateFeeSplit(feeAmount: number): { validatorAmount: number; burnAmount: number } {
    const split = this.getAdaptiveFeeSplit();
    return {
      validatorAmount: feeAmount * split.validatorShare,
      burnAmount: feeAmount * split.burnShare
    };
  }

  getStats(checkpointHeight: number): EmissionStats {
    const feeSplit = this.getAdaptiveFeeSplit();
    return {
      currentReward: this.getCheckpointReward(checkpointHeight),
      halvingEpoch: this.getHalvingEpoch(checkpointHeight),
      nextHalvingAt: this.getNextHalvingHeight(checkpointHeight),
      totalEmitted: this.totalEmitted,
      remainingToEmit: this.getRemainingToEmit(),
      circulatingSupply: this.getCirculatingSupply(),
      totalBurned: this.totalBurned,
      validatorFeePercent: feeSplit.validatorShare * 100,
      burnPercent: feeSplit.burnShare * 100,
    };
  }

  toJSON(): { totalEmitted: number; totalBurned: number } {
    return {
      totalEmitted: this.totalEmitted,
      totalBurned: this.totalBurned,
    };
  }

  static fromJSON(data: { totalEmitted?: number; totalBurned?: number }): EmissionService {
    return new EmissionService(data.totalEmitted ?? 0, data.totalBurned ?? 0);
  }
}

export interface SlashingServiceDeps {
  getStake: (address: string) => StakePosition | undefined;
  updateStake: (address: string, newAmount: number) => void;
  removeStake: (address: string) => void;
  updateBalance: (address: string, delta: number) => Promise<boolean>;
}

export class SlashingService {
  private slashEvents: SlashEvent[] = [];
  private unbondingQueue: UnbondingEntry[] = [];
  private livenessFailures: Map<string, { count: number; lastFailure: number }> = new Map();
  private nextSlashId = 1;

  constructor(private deps: SlashingServiceDeps) {}

  async slashValidator(
    validator: string,
    reason: SlashReason,
    checkpointHeight: number,
    details?: string
  ): Promise<SlashEvent | null> {
    const stake = this.deps.getStake(validator);
    if (!stake || stake.amount <= 0) {
      return null;
    }

    let percentToSlash: number;
    switch (reason) {
      case 'double_sign':
        percentToSlash = TOKENOMICS_CONFIG.SLASH_DOUBLE_SIGN_PERCENT;
        break;
      case 'invalid_checkpoint':
        percentToSlash = TOKENOMICS_CONFIG.SLASH_INVALID_CHECKPOINT_PERCENT;
        break;
      case 'invalid_proof':
        percentToSlash = TOKENOMICS_CONFIG.SLASH_INVALID_PROOF_PERCENT;
        break;
      case 'invalid_witness':
        percentToSlash = TOKENOMICS_CONFIG.SLASH_INVALID_WITNESS_PERCENT;
        break;
      case 'receipt_tampering':
        percentToSlash = TOKENOMICS_CONFIG.SLASH_RECEIPT_TAMPERING_PERCENT;
        break;
      case 'liveness_failure':
        percentToSlash = TOKENOMICS_CONFIG.SLASH_LIVENESS_PERCENT;
        break;
      case 'liveness_repeat':
        percentToSlash = TOKENOMICS_CONFIG.SLASH_LIVENESS_REPEAT_PERCENT;
        break;
      default:
        percentToSlash = 0.05;
    }

    const slashAmount = stake.amount * percentToSlash;
    const newStakeAmount = stake.amount - slashAmount;

    if (newStakeAmount > 0) {
      this.deps.updateStake(validator, newStakeAmount);
    } else {
      this.deps.removeStake(validator);
    }

    const event: SlashEvent = {
      id: `slash_${this.nextSlashId++}`,
      validator,
      reason,
      amount: slashAmount,
      percentSlashed: percentToSlash,
      checkpointHeight,
      timestamp: Date.now(),
      details,
    };

    this.slashEvents.push(event);
    if (this.slashEvents.length > 1000) {
      this.slashEvents = this.slashEvents.slice(-500);
    }

    return event;
  }

  async recordLivenessFailure(validator: string, checkpointHeight: number): Promise<SlashEvent | null> {
    const now = Date.now();
    const existing = this.livenessFailures.get(validator);
    
    if (existing) {
      const withinRepeatWindow = (now - existing.lastFailure) < TOKENOMICS_CONFIG.LIVENESS_REPEAT_WINDOW_MS;
      existing.count++;
      existing.lastFailure = now;
      
      if (existing.count >= TOKENOMICS_CONFIG.LIVENESS_MISS_THRESHOLD) {
        const reason = withinRepeatWindow && existing.count > TOKENOMICS_CONFIG.LIVENESS_MISS_THRESHOLD 
          ? 'liveness_repeat' 
          : 'liveness_failure';
        existing.count = 0;
        return this.slashValidator(validator, reason, checkpointHeight, 
          `Missed ${TOKENOMICS_CONFIG.LIVENESS_MISS_THRESHOLD} consecutive checkpoints`);
      }
    } else {
      this.livenessFailures.set(validator, { count: 1, lastFailure: now });
    }
    
    return null;
  }

  resetLivenessCounter(validator: string): void {
    this.livenessFailures.delete(validator);
  }

  startUnbonding(validator: string, amount: number): UnbondingEntry {
    const entry: UnbondingEntry = {
      validator,
      amount,
      startedAt: Date.now(),
      availableAt: Date.now() + TOKENOMICS_CONFIG.UNBONDING_PERIOD_MS,
      slashable: true,
    };
    this.unbondingQueue.push(entry);
    return entry;
  }

  async processUnbondingQueue(): Promise<number> {
    const now = Date.now();
    let released = 0;

    const completed: UnbondingEntry[] = [];
    const pending: UnbondingEntry[] = [];

    for (const entry of this.unbondingQueue) {
      if (now >= entry.availableAt && entry.slashable) {
        completed.push(entry);
      } else {
        pending.push(entry);
      }
    }

    for (const entry of completed) {
      await this.deps.updateBalance(entry.validator, entry.amount);
      released += entry.amount;
    }

    this.unbondingQueue = pending;
    return released;
  }

  slashUnbondingStake(validator: string, percent: number): number {
    let slashed = 0;
    for (const entry of this.unbondingQueue) {
      if (entry.validator === validator && entry.slashable) {
        const slashAmount = entry.amount * percent;
        entry.amount -= slashAmount;
        slashed += slashAmount;
      }
    }
    this.unbondingQueue = this.unbondingQueue.filter(e => e.amount > 0);
    return slashed;
  }

  getSlashEvents(limit = 50): SlashEvent[] {
    return this.slashEvents.slice(-limit);
  }

  getValidatorSlashHistory(validator: string): SlashEvent[] {
    return this.slashEvents.filter(e => e.validator === validator);
  }

  getUnbondingQueue(): UnbondingEntry[] {
    return [...this.unbondingQueue];
  }

  getUnbondingForValidator(validator: string): UnbondingEntry[] {
    return this.unbondingQueue.filter(e => e.validator === validator);
  }

  getTotalSlashed(): number {
    return this.slashEvents.reduce((sum, e) => sum + e.amount, 0);
  }

  toJSON(): TokenomicsSnapshot {
    return {
      totalEmitted: 0,
      totalBurned: 0,
      slashEvents: this.slashEvents,
      unbondingQueue: this.unbondingQueue,
      livenessFailures: Array.from(this.livenessFailures.entries()),
    };
  }

  static fromJSON(data: Partial<TokenomicsSnapshot>, deps: SlashingServiceDeps): SlashingService {
    const service = new SlashingService(deps);
    
    if (data.slashEvents) {
      service.slashEvents = data.slashEvents;
      service.nextSlashId = Math.max(...data.slashEvents.map(e => {
        const match = e.id.match(/slash_(\d+)/);
        return match ? parseInt(match[1]) : 0;
      })) + 1;
    }
    
    if (data.unbondingQueue) {
      service.unbondingQueue = data.unbondingQueue;
    }
    
    if (data.livenessFailures) {
      for (const [validator, info] of data.livenessFailures) {
        service.livenessFailures.set(validator, info);
      }
    }
    
    return service;
  }
}

export interface ValidatorWeightInfo {
  address: string;
  stakeAmount: number;
  ageWeight: number;
  missedCheckpoints: number;
}

export function calculateEffectiveAgeWeight(
  ageWeight: number,
  stakeAmount: number,
  missedCheckpoints: number
): number {
  if (stakeAmount < TOKENOMICS_CONFIG.MIN_BOND_FOR_AGE_WEIGHT) {
    return 0;
  }
  
  const decayFactor = Math.pow(
    1 - TOKENOMICS_CONFIG.AGE_DECAY_PER_MISS,
    missedCheckpoints
  );
  
  return ageWeight * decayFactor;
}

export function distributeCheckpointReward(
  reward: number,
  validators: ValidatorWeightInfo[]
): Map<string, number> {
  const distribution = new Map<string, number>();
  
  if (validators.length === 0 || reward <= 0) {
    return distribution;
  }

  const totalStake = validators.reduce((sum, v) => sum + v.stakeAmount, 0);
  
  const effectiveAgeWeights = validators.map(v => ({
    address: v.address,
    effectiveAge: calculateEffectiveAgeWeight(v.ageWeight, v.stakeAmount, v.missedCheckpoints)
  }));
  const totalEffectiveAge = effectiveAgeWeights.reduce((sum, v) => sum + v.effectiveAge, 0);

  const stakePool = reward * TOKENOMICS_CONFIG.STAKE_WEIGHT_PERCENT;
  const agePool = reward * TOKENOMICS_CONFIG.AGE_WEIGHT_PERCENT;

  for (let i = 0; i < validators.length; i++) {
    const validator = validators[i];
    let share = 0;
    
    if (totalStake > 0) {
      share += (validator.stakeAmount / totalStake) * stakePool;
    }
    
    if (totalEffectiveAge > 0) {
      share += (effectiveAgeWeights[i].effectiveAge / totalEffectiveAge) * agePool;
    }
    
    if (share > 0) {
      distribution.set(validator.address, share);
    }
  }

  return distribution;
}
