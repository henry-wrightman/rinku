const NODE_URL = process.env.RINKU_NODE_URL || "http://localhost:3001";
const FAUCET_URL = process.env.RINKU_FAUCET_URL || "http://localhost:3002";

interface StakingInfo {
  totalStaked: number;
  validators: { staker: string; amount: number }[];
  topStakers: { staker: string; amount: number }[];
  config: { minStakeAmount: number };
}

interface FaucetResponse {
  success: boolean;
  to: string;
  amount: number;
}

interface AccountsResponse {
  accounts: { fingerprint: string; balance: number }[];
}

interface StakeResult {
  success: boolean;
  position?: { staker: string; amount: number };
  error?: string;
}

interface StakingStatus {
  stakedAmount: number;
  isValidator: boolean;
  earnedRewards: number;
}

interface RewardsSummary {
  tipRewards: number;
  stakeRewards: number;
  witnessRewards: number;
  totalRewards: number;
  pendingRewards: number;
}

async function main() {
  console.log("=== Rinku Rewards & Staking Demo ===\n");

  console.log("1. Checking staking configuration...");
  const configRes = await fetch(`${NODE_URL}/api/rewards/config`);
  const config = await configRes.json();
  console.log("   Config:", JSON.stringify(config, null, 2));

  console.log("\n2. Getting staking overview...");
  const stakingRes = await fetch(`${NODE_URL}/api/staking`);
  const staking = (await stakingRes.json()) as StakingInfo;
  console.log(`   Total staked: ${staking.totalStaked}`);
  console.log(`   Active validators: ${staking.validators.length}`);
  console.log(`   Min stake amount: ${staking.config.minStakeAmount}`);

  console.log("\n3. Creating a test wallet via faucet...");
  let testAddress = "";
  try {
    const faucetRes = await fetch(`${FAUCET_URL}/api/faucet/request`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ amount: 500 }),
    });
    const faucetData = (await faucetRes.json()) as FaucetResponse;
    if (faucetData.success) {
      testAddress = faucetData.to;
      console.log(`   Created wallet: ${testAddress.slice(0, 16)}...`);
      console.log(`   Balance: ${faucetData.amount} RKU`);
    } else {
      console.log("   Faucet request failed, using existing account...");
      const accountsRes = await fetch(`${NODE_URL}/api/accounts`);
      const accountsData = (await accountsRes.json()) as AccountsResponse;
      const account = accountsData.accounts.find(
        (a) => a.balance >= 200 && a.fingerprint !== "faucet",
      );
      if (account) {
        testAddress = account.fingerprint;
        console.log(
          `   Using existing wallet: ${testAddress.slice(0, 16)}... with ${account.balance} RKU`,
        );
      } else {
        console.log("   No suitable account found for testing");
        return;
      }
    }
  } catch (e: unknown) {
    const error = e as Error;
    console.log(`   Faucet error: ${error.message}`);
    const accountsRes = await fetch(`${NODE_URL}/api/accounts`);
    const accountsData = (await accountsRes.json()) as AccountsResponse;
    const account = accountsData.accounts.find(
      (a) => a.balance >= 200 && a.fingerprint !== "faucet",
    );
    if (account) {
      testAddress = account.fingerprint;
      console.log(
        `   Using existing wallet: ${testAddress.slice(0, 16)}... with ${account.balance} RKU`,
      );
    } else {
      console.log("   No suitable account found for testing");
      return;
    }
  }

  console.log("\n4. Staking 200 RKU...");
  const stakeRes = await fetch(`${NODE_URL}/api/staking/stake`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ address: testAddress, amount: 200 }),
  });
  const stakeResult = (await stakeRes.json()) as StakeResult;
  if (stakeResult.success) {
    console.log(`   Stake successful!`);
    console.log(`   Position: ${JSON.stringify(stakeResult.position)}`);
  } else {
    console.log(`   Stake failed: ${stakeResult.error}`);
  }

  console.log("\n5. Checking staking status...");
  const statusRes = await fetch(`${NODE_URL}/api/staking/${testAddress}`);
  const status = (await statusRes.json()) as StakingStatus;
  console.log(`   Staked amount: ${status.stakedAmount}`);
  console.log(`   Is validator: ${status.isValidator}`);
  console.log(`   Earned rewards: ${status.earnedRewards}`);

  console.log("\n6. Checking rewards summary...");
  const rewardsRes = await fetch(`${NODE_URL}/api/rewards/${testAddress}`);
  const rewards = (await rewardsRes.json()) as RewardsSummary;
  console.log(`   Tip rewards: ${rewards.tipRewards}`);
  console.log(`   Stake rewards: ${rewards.stakeRewards}`);
  console.log(`   Witness rewards: ${rewards.witnessRewards}`);
  console.log(`   Total: ${rewards.totalRewards}`);
  console.log(`   Pending: ${rewards.pendingRewards}`);

  console.log("\n7. Updated staking overview...");
  const finalStakingRes = await fetch(`${NODE_URL}/api/staking`);
  const finalStaking = (await finalStakingRes.json()) as StakingInfo;
  console.log(`   Total staked: ${finalStaking.totalStaked}`);
  console.log(`   Active validators: ${finalStaking.validators.length}`);
  if (finalStaking.topStakers.length > 0) {
    console.log("   Top stakers:");
    for (const staker of finalStaking.topStakers.slice(0, 5)) {
      console.log(
        `     - ${staker.staker.slice(0, 12)}... : ${staker.amount} RKU`,
      );
    }
  }

  console.log("\n=== Demo Complete ===");
  console.log(
    '\nVisit the explorer and click "rewards" tab to see your staking status!',
  );
  console.log(
    "Rewards are automatically distributed when transactions are processed.",
  );
}

main().catch(console.error);
