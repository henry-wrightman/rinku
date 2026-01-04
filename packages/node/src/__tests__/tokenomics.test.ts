import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
  TOKENOMICS_CONFIG,
  EmissionService,
  SlashingService,
  calculateEffectiveAgeWeight,
  distributeCheckpointReward,
  type ValidatorWeightInfo,
  type SlashingServiceDeps
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

  describe('EmissionService Serialization', () => {
    it('should serialize and deserialize correctly', () => {
      const service = new EmissionService(1000, 50);
      service.recordEmission(200);
      service.recordBurn(25);
      
      const json = service.toJSON();
      expect(json.totalEmitted).toBe(1200);
      expect(json.totalBurned).toBe(75);
      
      const restored = EmissionService.fromJSON(json);
      expect(restored.getCirculatingSupply()).toBe(service.getCirculatingSupply());
    });

    it('should handle empty fromJSON', () => {
      const service = EmissionService.fromJSON({});
      expect(service.getCirculatingSupply()).toBe(TOKENOMICS_CONFIG.GENESIS_ALLOCATION);
    });
  });

  describe('SlashingService', () => {
    let deps: SlashingServiceDeps;
    let stakes: Map<string, { amount: number }>;

    beforeEach(() => {
      stakes = new Map([
        ['v1', { amount: 1000 }],
        ['v2', { amount: 500 }],
        ['v3', { amount: 100 }]
      ]);

      deps = {
        getStake: (address: string) => stakes.get(address),
        updateStake: (address: string, newAmount: number) => {
          stakes.set(address, { amount: newAmount });
        },
        removeStake: (address: string) => {
          stakes.delete(address);
        },
        updateBalance: vi.fn().mockResolvedValue(true)
      };
    });

    describe('slashValidator', () => {
      it('should slash for double_sign', async () => {
        const service = new SlashingService(deps);
        const event = await service.slashValidator('v1', 'double_sign', 100);
        
        expect(event).not.toBeNull();
        expect(event!.reason).toBe('double_sign');
        expect(event!.percentSlashed).toBe(TOKENOMICS_CONFIG.SLASH_DOUBLE_SIGN_PERCENT);
        expect(event!.amount).toBe(1000 * TOKENOMICS_CONFIG.SLASH_DOUBLE_SIGN_PERCENT);
      });

      it('should slash for invalid_checkpoint', async () => {
        const service = new SlashingService(deps);
        const event = await service.slashValidator('v1', 'invalid_checkpoint', 100);
        expect(event!.percentSlashed).toBe(TOKENOMICS_CONFIG.SLASH_INVALID_CHECKPOINT_PERCENT);
      });

      it('should slash for invalid_proof', async () => {
        const service = new SlashingService(deps);
        const event = await service.slashValidator('v1', 'invalid_proof', 100);
        expect(event!.percentSlashed).toBe(TOKENOMICS_CONFIG.SLASH_INVALID_PROOF_PERCENT);
      });

      it('should slash for invalid_witness', async () => {
        const service = new SlashingService(deps);
        const event = await service.slashValidator('v1', 'invalid_witness', 100);
        expect(event!.percentSlashed).toBe(TOKENOMICS_CONFIG.SLASH_INVALID_WITNESS_PERCENT);
      });

      it('should slash for receipt_tampering', async () => {
        const service = new SlashingService(deps);
        const event = await service.slashValidator('v1', 'receipt_tampering', 100);
        expect(event!.percentSlashed).toBe(TOKENOMICS_CONFIG.SLASH_RECEIPT_TAMPERING_PERCENT);
      });

      it('should slash for liveness_failure', async () => {
        const service = new SlashingService(deps);
        const event = await service.slashValidator('v1', 'liveness_failure', 100);
        expect(event!.percentSlashed).toBe(TOKENOMICS_CONFIG.SLASH_LIVENESS_PERCENT);
      });

      it('should slash for liveness_repeat', async () => {
        const service = new SlashingService(deps);
        const event = await service.slashValidator('v1', 'liveness_repeat', 100);
        expect(event!.percentSlashed).toBe(TOKENOMICS_CONFIG.SLASH_LIVENESS_REPEAT_PERCENT);
      });

      it('should update stake after slashing', async () => {
        const service = new SlashingService(deps);
        await service.slashValidator('v1', 'double_sign', 100);
        
        const newStake = stakes.get('v1')!.amount;
        expect(newStake).toBeLessThan(1000);
        expect(newStake).toBe(1000 * (1 - TOKENOMICS_CONFIG.SLASH_DOUBLE_SIGN_PERCENT));
      });

      it('should remove stake if slashed below remaining', async () => {
        stakes.set('tiny', { amount: 0.001 });
        const service = new SlashingService(deps);
        
        const event = await service.slashValidator('tiny', 'double_sign', 100);
        expect(event).not.toBeNull();
        
        const remainingStake = stakes.get('tiny');
        const remainingAmount = remainingStake?.amount ?? 0;
        expect(remainingAmount).toBeLessThan(0.001);
      });

      it('should return null for non-existent validator', async () => {
        const service = new SlashingService(deps);
        const event = await service.slashValidator('nonexistent', 'double_sign', 100);
        expect(event).toBeNull();
      });

      it('should include details in slash event', async () => {
        const service = new SlashingService(deps);
        const event = await service.slashValidator('v1', 'double_sign', 100, 'Test details');
        expect(event!.details).toBe('Test details');
      });

      it('should prune old events when limit exceeded', async () => {
        const service = new SlashingService(deps);
        
        for (let i = 0; i < 1001; i++) {
          stakes.set(`v${i}`, { amount: 1000 });
          await service.slashValidator(`v${i}`, 'liveness_failure', i);
        }
        
        const events = service.getSlashEvents(1000);
        expect(events.length).toBeLessThanOrEqual(500);
      });
    });

    describe('recordLivenessFailure', () => {
      it('should track first failure without slashing', async () => {
        const service = new SlashingService(deps);
        const event = await service.recordLivenessFailure('v1', 100);
        expect(event).toBeNull();
      });

      it('should slash after threshold failures', async () => {
        const service = new SlashingService(deps);
        
        for (let i = 0; i < TOKENOMICS_CONFIG.LIVENESS_MISS_THRESHOLD - 1; i++) {
          const event = await service.recordLivenessFailure('v1', 100 + i);
          expect(event).toBeNull();
        }
        
        const finalEvent = await service.recordLivenessFailure('v1', 200);
        expect(finalEvent).not.toBeNull();
        expect(finalEvent!.reason).toBe('liveness_failure');
      });

      it('should reset counter after slashing', async () => {
        const service = new SlashingService(deps);
        
        for (let i = 0; i < TOKENOMICS_CONFIG.LIVENESS_MISS_THRESHOLD; i++) {
          await service.recordLivenessFailure('v1', 100 + i);
        }
        
        const nextEvent = await service.recordLivenessFailure('v1', 300);
        expect(nextEvent).toBeNull();
      });
    });

    describe('resetLivenessCounter', () => {
      it('should reset validator liveness counter', async () => {
        const service = new SlashingService(deps);
        
        for (let i = 0; i < TOKENOMICS_CONFIG.LIVENESS_MISS_THRESHOLD - 1; i++) {
          await service.recordLivenessFailure('v1', 100 + i);
        }
        
        service.resetLivenessCounter('v1');
        
        const event = await service.recordLivenessFailure('v1', 200);
        expect(event).toBeNull();
      });
    });

    describe('unbonding queue', () => {
      it('should start unbonding', () => {
        const service = new SlashingService(deps);
        const entry = service.startUnbonding('v1', 500);
        
        expect(entry.validator).toBe('v1');
        expect(entry.amount).toBe(500);
        expect(entry.slashable).toBe(true);
        expect(entry.availableAt).toBeGreaterThan(entry.startedAt);
      });

      it('should process completed unbonding', async () => {
        const service = new SlashingService(deps);
        
        const entry = service.startUnbonding('v1', 500);
        entry.availableAt = Date.now() - 1000;
        
        const released = await service.processUnbondingQueue();
        expect(released).toBe(500);
        expect(deps.updateBalance).toHaveBeenCalledWith('v1', 500);
      });

      it('should not process pending unbonding', async () => {
        const service = new SlashingService(deps);
        service.startUnbonding('v1', 500);
        
        const released = await service.processUnbondingQueue();
        expect(released).toBe(0);
      });

      it('should slash unbonding stake', () => {
        const service = new SlashingService(deps);
        service.startUnbonding('v1', 1000);
        
        const slashed = service.slashUnbondingStake('v1', 0.5);
        expect(slashed).toBe(500);
        
        const remaining = service.getUnbondingForValidator('v1');
        expect(remaining[0].amount).toBe(500);
      });

      it('should remove entries slashed to zero', () => {
        const service = new SlashingService(deps);
        service.startUnbonding('v1', 100);
        
        const slashed = service.slashUnbondingStake('v1', 1.0);
        expect(slashed).toBe(100);
        
        const remaining = service.getUnbondingForValidator('v1');
        expect(remaining.length).toBe(0);
      });

      it('should get unbonding queue', () => {
        const service = new SlashingService(deps);
        service.startUnbonding('v1', 100);
        service.startUnbonding('v2', 200);
        
        const queue = service.getUnbondingQueue();
        expect(queue.length).toBe(2);
      });
    });

    describe('slash event queries', () => {
      it('should get recent slash events', async () => {
        const service = new SlashingService(deps);
        await service.slashValidator('v1', 'liveness_failure', 100);
        await service.slashValidator('v2', 'double_sign', 101);
        
        const events = service.getSlashEvents(10);
        expect(events.length).toBe(2);
      });

      it('should get validator slash history', async () => {
        const service = new SlashingService(deps);
        await service.slashValidator('v1', 'liveness_failure', 100);
        stakes.set('v1', { amount: 1000 });
        await service.slashValidator('v1', 'liveness_failure', 101);
        await service.slashValidator('v2', 'double_sign', 102);
        
        const v1History = service.getValidatorSlashHistory('v1');
        expect(v1History.length).toBe(2);
        
        const v2History = service.getValidatorSlashHistory('v2');
        expect(v2History.length).toBe(1);
      });

      it('should get total slashed', async () => {
        const service = new SlashingService(deps);
        await service.slashValidator('v1', 'liveness_failure', 100);
        const event1Amount = service.getSlashEvents()[0].amount;
        
        stakes.set('v1', { amount: 500 });
        await service.slashValidator('v1', 'liveness_failure', 101);
        const event2Amount = service.getSlashEvents()[1].amount;
        
        expect(service.getTotalSlashed()).toBeCloseTo(event1Amount + event2Amount, 5);
      });
    });

    describe('serialization', () => {
      it('should serialize to JSON', async () => {
        const service = new SlashingService(deps);
        await service.slashValidator('v1', 'liveness_failure', 100);
        service.startUnbonding('v2', 300);
        await service.recordLivenessFailure('v3', 101);
        
        const json = service.toJSON();
        expect(json.slashEvents.length).toBe(1);
        expect(json.unbondingQueue.length).toBe(1);
        expect(json.livenessFailures.length).toBe(1);
      });

      it('should deserialize from JSON', async () => {
        const service = new SlashingService(deps);
        await service.slashValidator('v1', 'liveness_failure', 100);
        service.startUnbonding('v2', 300);
        
        const json = service.toJSON();
        const restored = SlashingService.fromJSON(json, deps);
        
        expect(restored.getSlashEvents().length).toBe(1);
        expect(restored.getUnbondingQueue().length).toBe(1);
      });

      it('should restore slash ID counter', async () => {
        const service = new SlashingService(deps);
        await service.slashValidator('v1', 'liveness_failure', 100);
        await service.slashValidator('v2', 'liveness_failure', 101);
        
        const json = service.toJSON();
        const restored = SlashingService.fromJSON(json, deps);
        
        stakes.set('v3', { amount: 1000 });
        const newEvent = await restored.slashValidator('v3', 'double_sign', 200);
        
        expect(newEvent!.id).toMatch(/slash_3/);
      });

      it('should handle empty JSON gracefully', () => {
        const restored = SlashingService.fromJSON({}, deps);
        expect(restored.getSlashEvents().length).toBe(0);
        expect(restored.getUnbondingQueue().length).toBe(0);
      });

      it('should restore liveness failures', async () => {
        const service = new SlashingService(deps);
        await service.recordLivenessFailure('v1', 100);
        
        const json = service.toJSON();
        expect(json.livenessFailures.length).toBe(1);
        expect(json.livenessFailures[0][0]).toBe('v1');
        
        const restored = SlashingService.fromJSON(json, deps);
        
        for (let i = 0; i < TOKENOMICS_CONFIG.LIVENESS_MISS_THRESHOLD - 2; i++) {
          await restored.recordLivenessFailure('v1', 101 + i);
        }
        
        const finalEvent = await restored.recordLivenessFailure('v1', 200);
        expect(finalEvent).not.toBeNull();
      });
    });
  });
});
