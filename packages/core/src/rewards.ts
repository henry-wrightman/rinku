import type {
  TipReward,
  StakeReward,
  WitnessReward,
  Reward,
  RewardConfig,
  RewardsSummary,
  StakePosition,
  StakingStatus
} from './types.js';

export const DEFAULT_REWARD_CONFIG: RewardConfig = {
  tipRewardRate: 0.01,
  stakeRewardRate: 0.005,
  witnessRewardRate: 0.002,
  minStakeAmount: 100,
  unstakeCooldownMs: 24 * 60 * 60 * 1000
};

export function createTipReward(
  recipient: string,
  amount: number,
  txUrl: string,
  tipUrl: string
): TipReward {
  return {
    type: 'tip',
    recipient,
    amount,
    txUrl,
    tipUrl,
    timestamp: Date.now()
  };
}

export function createStakeReward(
  recipient: string,
  amount: number,
  validatedTxUrl: string
): StakeReward {
  return {
    type: 'stake',
    recipient,
    amount,
    validatedTxUrl,
    timestamp: Date.now()
  };
}

export function createWitnessReward(
  recipient: string,
  amount: number,
  witnessedTxUrl: string,
  referencedByUrl: string
): WitnessReward {
  return {
    type: 'witness',
    recipient,
    amount,
    witnessedTxUrl,
    referencedByUrl,
    timestamp: Date.now()
  };
}

export function createRewardsSummary(
  address: string,
  rewards: Reward[]
): RewardsSummary {
  const tipRewards = rewards
    .filter((r): r is TipReward => r.type === 'tip')
    .reduce((sum, r) => sum + r.amount, 0);

  const stakeRewards = rewards
    .filter((r): r is StakeReward => r.type === 'stake')
    .reduce((sum, r) => sum + r.amount, 0);

  const witnessRewards = rewards
    .filter((r): r is WitnessReward => r.type === 'witness')
    .reduce((sum, r) => sum + r.amount, 0);

  return {
    address,
    tipRewards,
    stakeRewards,
    witnessRewards,
    totalRewards: tipRewards + stakeRewards + witnessRewards,
    pendingRewards: 0,
    rewardHistory: rewards
  };
}

export function createStakePosition(
  staker: string,
  amount: number
): StakePosition {
  const now = Date.now();
  return {
    staker,
    amount,
    stakedAt: now,
    lastRewardAt: now
  };
}

export function createStakingStatus(
  address: string,
  position: StakePosition | null,
  earnedRewards: number,
  config: RewardConfig
): StakingStatus {
  if (!position) {
    return {
      address,
      stakedAmount: 0,
      isValidator: false,
      stakedAt: null,
      earnedRewards,
      canUnstakeAt: null
    };
  }

  return {
    address,
    stakedAmount: position.amount,
    isValidator: position.amount >= config.minStakeAmount,
    stakedAt: position.stakedAt,
    earnedRewards,
    canUnstakeAt: position.stakedAt + config.unstakeCooldownMs
  };
}

export function calculateTipRewardAmount(
  config: RewardConfig,
  txAmount: number
): number {
  return Math.floor(txAmount * config.tipRewardRate);
}

export function calculateStakeRewardAmount(
  config: RewardConfig,
  stakedAmount: number,
  txAmount: number
): number {
  return Math.floor(stakedAmount * config.stakeRewardRate * (txAmount / 1000));
}

export function calculateWitnessRewardAmount(
  config: RewardConfig,
  txAmount: number
): number {
  return Math.floor(txAmount * config.witnessRewardRate);
}
