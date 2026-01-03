import { useState, useEffect } from "react";

interface RewardsSummary {
  address: string;
  tipRewards: number;
  stakeRewards: number;
  witnessRewards: number;
  totalRewards: number;
  pendingRewards: number;
  rewardHistory: Reward[];
}

interface Reward {
  type: "tip" | "stake" | "witness";
  recipient: string;
  amount: number;
  timestamp: number;
}

interface StakingStatus {
  address: string;
  stakedAmount: number;
  isValidator: boolean;
  stakedAt: number | null;
  earnedRewards: number;
  canUnstakeAt: number | null;
}

interface StakingInfo {
  totalStaked: number;
  validators: { staker: string; amount: number; stakedAt: number }[];
  topStakers: { staker: string; amount: number; stakedAt: number }[];
  config: {
    tipRewardRate: number;
    stakeRewardRate: number;
    witnessRewardRate: number;
    minStakeAmount: number;
    unstakeCooldownMs: number;
  };
}

const NODE_URL = "/api";

export function RewardsTab() {
  const [address, setAddress] = useState("");
  const [rewards, setRewards] = useState<RewardsSummary | null>(null);
  const [staking, setStaking] = useState<StakingStatus | null>(null);
  const [stakingInfo, setStakingInfo] = useState<StakingInfo | null>(null);
  const [stakeAmount, setStakeAmount] = useState(100);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);

  const fetchStakingInfo = async () => {
    try {
      const res = await fetch(`${NODE_URL}/staking`);
      const data = await res.json();
      setStakingInfo(data);
    } catch (e) {
      console.error("Failed to fetch staking info:", e);
    }
  };

  useEffect(() => {
    fetchStakingInfo();
    const interval = setInterval(fetchStakingInfo, 5000);
    return () => clearInterval(interval);
  }, []);

  const fetchRewards = async () => {
    if (!address) return;
    setLoading(true);
    setError(null);

    try {
      const [rewardsRes, stakingRes] = await Promise.all([
        fetch(`${NODE_URL}/rewards/${address}`),
        fetch(`${NODE_URL}/staking/${address}`),
      ]);

      const rewardsData = await rewardsRes.json();
      const stakingData = await stakingRes.json();

      setRewards(rewardsData);
      setStaking(stakingData);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  };

  const handleStake = async () => {
    if (!address || stakeAmount <= 0) return;
    setError(null);
    setResult(null);

    try {
      const res = await fetch(`${NODE_URL}/staking/stake`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ address, amount: stakeAmount }),
      });

      const data = await res.json();
      if (data.success) {
        setResult(`Staked ${stakeAmount} RKU successfully`);
        fetchRewards();
        fetchStakingInfo();
      } else {
        setError(data.error || "Staking failed");
      }
    } catch (e: any) {
      setError(e.message);
    }
  };

  const handleUnstake = async () => {
    if (!address) return;
    setError(null);
    setResult(null);

    try {
      const res = await fetch(`${NODE_URL}/staking/unstake`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ address }),
      });

      const data = await res.json();
      if (data.success) {
        setResult(`Unstaked ${data.amount} RKU successfully`);
        fetchRewards();
        fetchStakingInfo();
      } else {
        setError(data.error || "Unstaking failed");
      }
    } catch (e: any) {
      setError(e.message);
    }
  };

  const handleClaim = async () => {
    if (!address) return;
    setError(null);
    setResult(null);

    try {
      const res = await fetch(`${NODE_URL}/rewards/${address}/claim`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
      });

      const data = await res.json();
      if (data.success) {
        setResult(`Claimed ${data.amount} reward RKU`);
        fetchRewards();
      } else {
        setError("No rewards to claim");
      }
    } catch (e: any) {
      setError(e.message);
    }
  };

  const formatTime = (ts: number) => {
    const d = new Date(ts);
    return d.toLocaleString();
  };

  return (
    <div className="rewards-tab">
      <div className="section">
        <h3>network staking</h3>
        {stakingInfo && (
          <div className="staking-overview">
            <div className="stat-row">
              <span>total staked:</span>
              <span className="value">{stakingInfo.totalStaked} RKU</span>
            </div>
            <div className="stat-row">
              <span>active validators:</span>
              <span className="value">{stakingInfo.validators.length}</span>
            </div>
            <div className="stat-row">
              <span>min stake:</span>
              <span className="value">
                {stakingInfo.config.minStakeAmount} RKU
              </span>
            </div>
            <div className="stat-row">
              <span>tip reward rate:</span>
              <span className="value">
                {(stakingInfo.config.tipRewardRate * 100).toFixed(1)}%
              </span>
            </div>
            <div className="stat-row">
              <span>stake reward rate:</span>
              <span className="value">
                {(stakingInfo.config.stakeRewardRate * 100).toFixed(2)}%
              </span>
            </div>
            <div className="stat-row">
              <span>witness reward rate:</span>
              <span className="value">
                {(stakingInfo.config.witnessRewardRate * 100).toFixed(2)}%
              </span>
            </div>

            {stakingInfo.topStakers.length > 0 && (
              <div className="top-stakers">
                <h4>top stakers</h4>
                {stakingInfo.topStakers.map((s, i) => (
                  <div key={i} className="staker-row">
                    <span className="mono">{s.staker.slice(0, 12)}...</span>
                    <span className="value">{s.amount} RKU</span>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      <div className="section">
        <h3>your rewards</h3>
        <div className="form-row">
          <input
            type="text"
            placeholder="your wallet address (fingerprint)"
            value={address}
            onChange={(e) => setAddress(e.target.value)}
          />
          <button onClick={fetchRewards} disabled={!address || loading}>
            {loading ? "loading..." : "lookup"}
          </button>
        </div>

        {rewards && (
          <div className="rewards-summary">
            <div className="rewards-breakdown">
              <div className="reward-type">
                <span className="label">tip rewards</span>
                <span className="amount">{rewards.tipRewards}</span>
              </div>
              <div className="reward-type">
                <span className="label">stake rewards</span>
                <span className="amount">{rewards.stakeRewards}</span>
              </div>
              <div className="reward-type">
                <span className="label">witness rewards</span>
                <span className="amount">{rewards.witnessRewards}</span>
              </div>
              <div className="reward-type total">
                <span className="label">total earned</span>
                <span className="amount">{rewards.totalRewards}</span>
              </div>
              <div className="reward-type pending">
                <span className="label">pending</span>
                <span className="amount">{rewards.pendingRewards}</span>
                {rewards.pendingRewards > 0 && (
                  <button className="claim-btn" onClick={handleClaim}>
                    claim
                  </button>
                )}
              </div>
            </div>

            {rewards.rewardHistory.length > 0 && (
              <div className="reward-history">
                <h4>recent rewards</h4>
                {rewards.rewardHistory
                  .slice(-10)
                  .reverse()
                  .map((r, i) => (
                    <div key={i} className="history-row">
                      <span className="type">{r.type}</span>
                      <span className="amount">+{r.amount}</span>
                      <span className="time">{formatTime(r.timestamp)}</span>
                    </div>
                  ))}
              </div>
            )}
          </div>
        )}
      </div>

      <div className="section">
        <h3>staking</h3>
        {staking && (
          <div className="staking-status">
            <div className="stat-row">
              <span>staked amount:</span>
              <span className="value">{staking.stakedAmount} RKU</span>
            </div>
            <div className="stat-row">
              <span>validator status:</span>
              <span className={`value ${staking.isValidator ? "active" : ""}`}>
                {staking.isValidator ? "active" : "not staking"}
              </span>
            </div>
            {staking.stakedAt && (
              <div className="stat-row">
                <span>staked since:</span>
                <span className="value">{formatTime(staking.stakedAt)}</span>
              </div>
            )}
            {staking.canUnstakeAt && (
              <div className="stat-row">
                <span>can unstake at:</span>
                <span className="value">
                  {formatTime(staking.canUnstakeAt)}
                </span>
              </div>
            )}
          </div>
        )}

        <div className="stake-form">
          <div className="form-row">
            <input
              type="number"
              placeholder="amount to stake"
              value={stakeAmount}
              onChange={(e) => setStakeAmount(parseInt(e.target.value) || 0)}
              min={stakingInfo?.config.minStakeAmount || 100}
            />
            <button
              onClick={handleStake}
              disabled={!address || stakeAmount <= 0}
            >
              stake
            </button>
            {staking?.stakedAmount && staking.stakedAmount > 0 && (
              <button onClick={handleUnstake} className="secondary">
                unstake
              </button>
            )}
          </div>
        </div>
      </div>

      {error && <div className="error">{error}</div>}
      {result && <div className="success">{result}</div>}
    </div>
  );
}
