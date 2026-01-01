import {
  type Reward,
  type TipReward,
  type StakeReward,
  type WitnessReward,
  type RewardConfig,
  type RewardsSummary,
  type StakePosition,
  type StakingStatus,
  type DAGNode,
  type AccountState,
  DEFAULT_REWARD_CONFIG,
  createTipReward,
  createStakeReward,
  createWitnessReward,
  createRewardsSummary,
  createStakePosition,
  createStakingStatus,
  calculateTipRewardAmount,
  calculateStakeRewardAmount,
  calculateWitnessRewardAmount
} from '@rinku/core';

export interface RewardsServiceDeps {
  getDAGNodeByUrl: (url: string) => DAGNode | undefined;
  getAccount: (address: string) => AccountState | undefined;
  updateBalance: (address: string, delta: number) => Promise<boolean>;
}

export class RewardsService {
  private rewards: Map<string, Reward[]> = new Map();
  private stakes: Map<string, StakePosition> = new Map();
  private pendingRewards: Map<string, number> = new Map();
  private witnessedTxs: Set<string> = new Set();
  private config: RewardConfig;

  constructor(
    private deps: RewardsServiceDeps,
    config?: Partial<RewardConfig>
  ) {
    this.config = { ...DEFAULT_REWARD_CONFIG, ...config };
  }

  getConfig(): RewardConfig {
    return { ...this.config };
  }

  processTipRewards(txUrl: string, tipUrls: string[], txAmount: number): TipReward[] {
    const rewards: TipReward[] = [];
    const rewardAmount = calculateTipRewardAmount(this.config, txAmount);

    if (rewardAmount <= 0) return rewards;

    for (const tipUrl of tipUrls) {
      const tipNode = this.deps.getDAGNodeByUrl(tipUrl);
      if (!tipNode) continue;

      const recipient = tipNode.tx.from;
      const reward = createTipReward(recipient, rewardAmount, txUrl, tipUrl);
      
      this.addReward(recipient, reward);
      rewards.push(reward);
    }

    return rewards;
  }

  processStakeRewards(txUrl: string, txAmount: number): StakeReward[] {
    const rewards: StakeReward[] = [];
    const validators = this.getActiveValidators();

    if (validators.length === 0) return rewards;

    for (const stake of validators) {
      const rewardAmount = calculateStakeRewardAmount(
        this.config,
        stake.amount,
        txAmount
      );

      if (rewardAmount <= 0) continue;

      const reward = createStakeReward(stake.staker, rewardAmount, txUrl);
      this.addReward(stake.staker, reward);
      stake.lastRewardAt = Date.now();
      rewards.push(reward);
    }

    return rewards;
  }

  processWitnessRewards(
    referencingTxUrl: string,
    referencedTxUrl: string,
    txAmount: number
  ): WitnessReward | null {
    const key = `${referencingTxUrl}:${referencedTxUrl}`;
    if (this.witnessedTxs.has(key)) return null;

    const referencedNode = this.deps.getDAGNodeByUrl(referencedTxUrl);
    if (!referencedNode) return null;

    const rewardAmount = calculateWitnessRewardAmount(this.config, txAmount);
    if (rewardAmount <= 0) return null;

    const recipient = referencedNode.tx.from;
    const reward = createWitnessReward(
      recipient,
      rewardAmount,
      referencedTxUrl,
      referencingTxUrl
    );

    this.addReward(recipient, reward);
    this.witnessedTxs.add(key);

    return reward;
  }

  processTransactionRewards(
    txUrl: string,
    tipUrls: string[],
    txAmount: number
  ): { tipRewards: TipReward[]; stakeRewards: StakeReward[]; witnessRewards: WitnessReward[] } {
    const tipRewards = this.processTipRewards(txUrl, tipUrls, txAmount);
    const stakeRewards = this.processStakeRewards(txUrl, txAmount);
    
    const witnessRewards: WitnessReward[] = [];
    for (const tipUrl of tipUrls) {
      const reward = this.processWitnessRewards(txUrl, tipUrl, txAmount);
      if (reward) witnessRewards.push(reward);
    }

    return { tipRewards, stakeRewards, witnessRewards };
  }

  async stake(address: string, amount: number): Promise<{ success: boolean; error?: string; position?: StakePosition }> {
    if (amount < this.config.minStakeAmount) {
      return {
        success: false,
        error: `Minimum stake amount is ${this.config.minStakeAmount}`
      };
    }

    const account = this.deps.getAccount(address);
    if (!account || account.balance < amount) {
      return { success: false, error: 'Insufficient balance' };
    }

    const debited = await this.deps.updateBalance(address, -amount);
    if (!debited) {
      return { success: false, error: 'Failed to debit balance' };
    }

    const existingStake = this.stakes.get(address);
    if (existingStake) {
      existingStake.amount += amount;
      existingStake.stakedAt = Date.now();
      return { success: true, position: existingStake };
    }

    const position = createStakePosition(address, amount);
    this.stakes.set(address, position);

    return { success: true, position };
  }

  async unstake(address: string): Promise<{ success: boolean; error?: string; amount?: number }> {
    const position = this.stakes.get(address);
    if (!position) {
      return { success: false, error: 'No stake found' };
    }

    const canUnstakeAt = position.stakedAt + this.config.unstakeCooldownMs;
    if (Date.now() < canUnstakeAt) {
      const remainingMs = canUnstakeAt - Date.now();
      const remainingHours = Math.ceil(remainingMs / (60 * 60 * 1000));
      return {
        success: false,
        error: `Cooldown not complete. ${remainingHours} hours remaining.`
      };
    }

    const amount = position.amount;
    
    const credited = await this.deps.updateBalance(address, amount);
    if (!credited) {
      return { success: false, error: 'Failed to credit balance' };
    }
    
    this.stakes.delete(address);

    return { success: true, amount };
  }

  getStakingStatus(address: string): StakingStatus {
    const position = this.stakes.get(address) || null;
    const rewards = this.rewards.get(address) || [];
    const stakeRewardsTotal = rewards
      .filter((r): r is StakeReward => r.type === 'stake')
      .reduce((sum, r) => sum + r.amount, 0);

    return createStakingStatus(address, position, stakeRewardsTotal, this.config);
  }

  getRewardsSummary(address: string): RewardsSummary {
    const rewards = this.rewards.get(address) || [];
    return createRewardsSummary(address, rewards);
  }

  getActiveValidators(): StakePosition[] {
    return Array.from(this.stakes.values())
      .filter(s => s.amount >= this.config.minStakeAmount);
  }

  getTotalStaked(): number {
    return Array.from(this.stakes.values())
      .reduce((sum, s) => sum + s.amount, 0);
  }

  getTopStakers(limit: number = 10): StakePosition[] {
    return Array.from(this.stakes.values())
      .sort((a, b) => b.amount - a.amount)
      .slice(0, limit);
  }

  async claimRewards(address: string): Promise<{ success: boolean; amount: number }> {
    const pending = this.pendingRewards.get(address) || 0;
    if (pending <= 0) {
      return { success: false, amount: 0 };
    }

    const credited = await this.deps.updateBalance(address, pending);
    if (!credited) {
      return { success: false, amount: 0 };
    }

    this.pendingRewards.set(address, 0);
    return { success: true, amount: pending };
  }

  private addReward(address: string, reward: Reward): void {
    const existing = this.rewards.get(address) || [];
    existing.push(reward);
    this.rewards.set(address, existing);

    const pending = this.pendingRewards.get(address) || 0;
    this.pendingRewards.set(address, pending + reward.amount);
  }

  toJSON(): object {
    return {
      rewards: Array.from(this.rewards.entries()),
      stakes: Array.from(this.stakes.entries()),
      pendingRewards: Array.from(this.pendingRewards.entries()),
      witnessedTxs: Array.from(this.witnessedTxs),
      config: this.config
    };
  }

  static fromJSON(data: any, deps: RewardsServiceDeps): RewardsService {
    const service = new RewardsService(deps, data.config);

    if (data.rewards) {
      for (const [address, rewards] of data.rewards) {
        service.rewards.set(address, rewards);
      }
    }

    if (data.stakes) {
      for (const [address, stake] of data.stakes) {
        service.stakes.set(address, stake);
      }
    }

    if (data.pendingRewards) {
      for (const [address, amount] of data.pendingRewards) {
        service.pendingRewards.set(address, amount);
      }
    }

    if (data.witnessedTxs) {
      for (const key of data.witnessedTxs) {
        service.witnessedTxs.add(key);
      }
    }

    return service;
  }
}
