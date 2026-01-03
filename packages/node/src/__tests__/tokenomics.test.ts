import { describe, it, expect } from 'vitest';
import {
  TOKENOMICS_CONFIG,
  EmissionService,
  calculateEffectiveAgeWeight,
  distributeCheckpointReward,
  type ValidatorWeightInfo
} from '../tokenomics.js';

describe('Tokenomics Module', () => {
  describe('TOKENOMICS_CONFIG', () => {
    it('should have extended halving interval (~18 months)', () => {
      expect(TOKENOMICS_CONFIG.HALVING_INTERVAL).toBe(3_150_000);
      const checkpointsPerDay = (24 * 60 * 60) / 15;
      const daysPerHalving = TOKENOMICS_CONFIG.HALVING_INTERVAL / checkpointsPerDay;
      const monthsPerHalving = daysPerHalving / 30;
      expect(monthsPerHalving).toBeGreaterThan(17);
      expect(monthsPerHalving).toBeLessThan(19);
    });

    it('should have validator fee floor at 70%', () => {
      expect(TOKENOMICS_CONFIG.VALIDATOR_FEE_FLOOR_PERCENT).toBe(0.70);
    });

    it('should have burn ceiling at 30%', () => {
      expect(TOKENOMICS_CONFIG.BURN_CEILING_PERCENT).toBe(0.30);
    });

    it('should have minimum bond for age weight', () => {
      expect(TOKENOMICS_CONFIG.MIN_BOND_FOR_AGE_WEIGHT).toBe(100);
    });

    it('should have age decay per miss at 10%', () => {
      expect(TOKENOMICS_CONFIG.AGE_DECAY_PER_MISS).toBe(0.10);
    });
  });

  describe('EmissionService', () => {
    describe('getCheckpointReward', () => {
      it('should return initial reward for epoch 0', () => {
        const service = new EmissionService();
        expect(service.getCheckpointReward(0)).toBe(3.932411);
        expect(service.getCheckpointReward(1000)).toBe(3.932411);
      });

      it('should halve reward after halving interval using floor(µRKU)', () => {
        const service = new EmissionService();
        expect(service.getCheckpointReward(TOKENOMICS_CONFIG.HALVING_INTERVAL)).toBe(1.966205);
        expect(service.getCheckpointReward(TOKENOMICS_CONFIG.HALVING_INTERVAL * 2)).toBe(0.983102);
        expect(service.getCheckpointReward(TOKENOMICS_CONFIG.HALVING_INTERVAL * 3)).toBe(0.491551);
        expect(service.getCheckpointReward(TOKENOMICS_CONFIG.HALVING_INTERVAL * 4)).toBe(0.245775);
      });

      it('should not go below minimum reward', () => {
        const service = new EmissionService();
        const veryHigh = TOKENOMICS_CONFIG.HALVING_INTERVAL * 10;
        expect(service.getCheckpointReward(veryHigh)).toBe(TOKENOMICS_CONFIG.MIN_CHECKPOINT_REWARD);
      });
    });

    describe('getAdaptiveFeeSplit', () => {
      it('should favor validators early (low circulating supply)', () => {
        const service = new EmissionService(0, 0);
        const split = service.getAdaptiveFeeSplit();
        expect(split.validatorShare).toBeGreaterThanOrEqual(0.70);
        expect(split.burnShare).toBeLessThanOrEqual(0.30);
      });

      it('should increase burn as supply grows toward target', () => {
        const halfWayEmitted = (TOKENOMICS_CONFIG.MAX_SUPPLY - TOKENOMICS_CONFIG.GENESIS_ALLOCATION) * 0.4;
        const service = new EmissionService(halfWayEmitted, 0);
        const split = service.getAdaptiveFeeSplit();
        expect(split.burnShare).toBeGreaterThan(0);
        expect(split.validatorShare + split.burnShare).toBeCloseTo(1, 5);
      });

      it('should cap burn at ceiling when supply hits target', () => {
        const fullEmitted = TOKENOMICS_CONFIG.MAX_SUPPLY - TOKENOMICS_CONFIG.GENESIS_ALLOCATION;
        const service = new EmissionService(fullEmitted, 0);
        const split = service.getAdaptiveFeeSplit();
        expect(split.burnShare).toBeCloseTo(TOKENOMICS_CONFIG.BURN_CEILING_PERCENT, 5);
        expect(split.validatorShare).toBeCloseTo(1 - TOKENOMICS_CONFIG.BURN_CEILING_PERCENT, 5);
      });
    });

    describe('calculateFeeSplit', () => {
      it('should split fee correctly', () => {
        const service = new EmissionService(0, 0);
        const { validatorAmount, burnAmount } = service.calculateFeeSplit(100);
        expect(validatorAmount + burnAmount).toBeCloseTo(100, 5);
        expect(validatorAmount).toBeGreaterThanOrEqual(70);
      });
    });

    describe('getStats', () => {
      it('should include fee split percentages', () => {
        const service = new EmissionService(0, 0);
        const stats = service.getStats(100);
        expect(stats.validatorFeePercent).toBeDefined();
        expect(stats.burnPercent).toBeDefined();
        expect(stats.validatorFeePercent + stats.burnPercent).toBeCloseTo(100, 5);
      });
    });
  });

  describe('calculateEffectiveAgeWeight', () => {
    it('should return 0 if stake below minimum', () => {
      const result = calculateEffectiveAgeWeight(100, 50, 0);
      expect(result).toBe(0);
    });

    it('should return full age weight with no missed checkpoints', () => {
      const result = calculateEffectiveAgeWeight(100, 1000, 0);
      expect(result).toBe(100);
    });

    it('should decay age weight with missed checkpoints', () => {
      const full = calculateEffectiveAgeWeight(100, 1000, 0);
      const oneMiss = calculateEffectiveAgeWeight(100, 1000, 1);
      const twoMiss = calculateEffectiveAgeWeight(100, 1000, 2);
      
      expect(oneMiss).toBeLessThan(full);
      expect(twoMiss).toBeLessThan(oneMiss);
      expect(oneMiss).toBeCloseTo(90, 1);
      expect(twoMiss).toBeCloseTo(81, 1);
    });

    it('should approach zero with many misses', () => {
      const result = calculateEffectiveAgeWeight(100, 1000, 20);
      expect(result).toBeLessThan(15);
    });
  });

  describe('distributeCheckpointReward', () => {
    const testReward = TOKENOMICS_CONFIG.INITIAL_CHECKPOINT_REWARD;

    it('should return empty map for empty validators', () => {
      const result = distributeCheckpointReward(testReward, []);
      expect(result.size).toBe(0);
    });

    it('should distribute reward to single validator', () => {
      const validators: ValidatorWeightInfo[] = [
        { address: 'v1', stakeAmount: 1000, ageWeight: 50, missedCheckpoints: 0 }
      ];
      const result = distributeCheckpointReward(testReward, validators);
      expect(result.get('v1')).toBeCloseTo(testReward, 5);
    });

    it('should distribute proportionally by stake (70%) and age (30%)', () => {
      const validators: ValidatorWeightInfo[] = [
        { address: 'v1', stakeAmount: 1000, ageWeight: 100, missedCheckpoints: 0 },
        { address: 'v2', stakeAmount: 1000, ageWeight: 100, missedCheckpoints: 0 }
      ];
      const result = distributeCheckpointReward(testReward, validators);
      expect(result.get('v1')).toBeCloseTo(testReward / 2, 5);
      expect(result.get('v2')).toBeCloseTo(testReward / 2, 5);
    });

    it('should favor higher stakes', () => {
      const validators: ValidatorWeightInfo[] = [
        { address: 'whale', stakeAmount: 9000, ageWeight: 100, missedCheckpoints: 0 },
        { address: 'small', stakeAmount: 1000, ageWeight: 100, missedCheckpoints: 0 }
      ];
      const result = distributeCheckpointReward(testReward, validators);
      expect(result.get('whale')!).toBeGreaterThan(result.get('small')!);
    });

    it('should not give age weight to validators below min bond', () => {
      const validators: ValidatorWeightInfo[] = [
        { address: 'bonded', stakeAmount: 1000, ageWeight: 100, missedCheckpoints: 0 },
        { address: 'unbonded', stakeAmount: 50, ageWeight: 100, missedCheckpoints: 0 }
      ];
      const result = distributeCheckpointReward(testReward, validators);
      expect(result.get('bonded')!).toBeGreaterThan(result.get('unbonded')!);
    });

    it('should decay age weight for missed checkpoints', () => {
      const validators: ValidatorWeightInfo[] = [
        { address: 'active', stakeAmount: 1000, ageWeight: 100, missedCheckpoints: 0 },
        { address: 'absent', stakeAmount: 1000, ageWeight: 100, missedCheckpoints: 5 }
      ];
      const result = distributeCheckpointReward(testReward, validators);
      expect(result.get('active')!).toBeGreaterThan(result.get('absent')!);
    });
  });
});
