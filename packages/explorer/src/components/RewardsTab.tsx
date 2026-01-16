import { useState, useEffect } from "react";
import { 
  createSignedTransaction, 
  generateKeyPair, 
  serializeKeyPair, 
  deserializeKeyPair, 
  validateSerializedKey,
  getFingerprint,
  type SerializedKeyPair 
} from "../crypto";

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
const WALLET_STORAGE_KEY = "rinku_wallet";

export function RewardsTab() {
  const [address, setAddress] = useState("");
  const [keyInput, setKeyInput] = useState("");
  const [keyPair, setKeyPair] = useState<SerializedKeyPair | null>(null);
  const [walletReady, setWalletReady] = useState(false);
  const [showPrivateKey, setShowPrivateKey] = useState(false);
  const [rewards, setRewards] = useState<RewardsSummary | null>(null);
  const [staking, setStaking] = useState<StakingStatus | null>(null);
  const [stakingInfo, setStakingInfo] = useState<StakingInfo | null>(null);
  const [stakeAmount, setStakeAmount] = useState(100);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);

  useEffect(() => {
    const stored = localStorage.getItem(WALLET_STORAGE_KEY);
    if (stored && validateSerializedKey(stored)) {
      try {
        const kp = deserializeKeyPair(stored);
        setKeyPair(kp);
        setWalletReady(true);
        setAddress(kp.fingerprint);
      } catch (e) {
        console.error("Failed to load stored wallet:", e);
      }
    }
  }, []);

  const handleImportKey = () => {
    setError(null);
    if (!keyInput.trim()) {
      setError("Please paste a key");
      return;
    }
    
    if (!validateSerializedKey(keyInput)) {
      setError("Invalid key format. Paste the full JSON key from CLI (including publicKey, privateKey, and fingerprint).");
      return;
    }
    
    try {
      const kp = deserializeKeyPair(keyInput);
      setKeyPair(kp);
      setWalletReady(true);
      setAddress(kp.fingerprint);
      localStorage.setItem(WALLET_STORAGE_KEY, keyInput);
      setResult(`Wallet imported! Address: ${kp.fingerprint.slice(0, 16)}...`);
      setKeyInput("");
    } catch (e: any) {
      setError("Failed to import key: " + e.message);
    }
  };

  const handleGenerateWallet = async () => {
    setError(null);
    try {
      const kp = await generateKeyPair();
      const serialized = serializeKeyPair(kp);
      localStorage.setItem(WALLET_STORAGE_KEY, serialized);
      setKeyPair(kp);
      setWalletReady(true);
      setAddress(kp.fingerprint);
      setShowPrivateKey(true);
      setResult(`Wallet created! Address: ${kp.fingerprint.slice(0, 16)}... SAVE YOUR KEY!`);
    } catch (e: any) {
      setError(e.message);
    }
  };

  const handleClearWallet = () => {
    localStorage.removeItem(WALLET_STORAGE_KEY);
    setKeyPair(null);
    setWalletReady(false);
    setAddress("");
    setRewards(null);
    setStaking(null);
    setResult("Wallet cleared");
  };

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

  useEffect(() => {
    if (address) {
      fetchRewards();
    }
  }, [address]);

  const handleStake = async () => {
    if (!walletReady || !keyPair) {
      setError("Set up a wallet first");
      return;
    }
    if (stakeAmount <= 0) {
      setError("Invalid stake amount");
      return;
    }
    setError(null);
    setResult(null);

    try {
      const tipsRes = await fetch(`${NODE_URL}/tips`);
      const tips = await tipsRes.json();
      const parents = (tips as string[]).slice(0, 2).map((h: string) => `rinku://tx/h/${h}`);

      const accountRes = await fetch(`${NODE_URL}/account/${keyPair.fingerprint}`);
      const account = await accountRes.json();
      const nonce = (account.nonce || 0) + 1;

      const signedTx = await createSignedTransaction(keyPair, {
        to: keyPair.fingerprint,
        amount: stakeAmount,
        nonce,
        parents,
        kind: "stake",
        gasPrice: 0.001,
      });

      const res = await fetch(`${NODE_URL}/tx`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(signedTx),
      });

      const data = await res.json();
      if (res.ok && data.hash) {
        setResult(`Staked ${stakeAmount} RKU (tx: ${data.hash.slice(0, 12)}...)`);
        setTimeout(() => {
          fetchRewards();
          fetchStakingInfo();
        }, 1000);
      } else {
        setError(data.error || "Staking failed");
      }
    } catch (e: any) {
      setError(e.message);
    }
  };

  const handleUnstake = async () => {
    if (!walletReady || !keyPair) {
      setError("Set up a wallet first");
      return;
    }
    if (!staking?.stakedAmount || staking.stakedAmount <= 0) {
      setError("No stake to unstake");
      return;
    }
    setError(null);
    setResult(null);

    try {
      const tipsRes = await fetch(`${NODE_URL}/tips`);
      const tips = await tipsRes.json();
      const parents = (tips as string[]).slice(0, 2).map((h: string) => `rinku://tx/h/${h}`);

      const accountRes = await fetch(`${NODE_URL}/account/${keyPair.fingerprint}`);
      const account = await accountRes.json();
      const nonce = (account.nonce || 0) + 1;

      const signedTx = await createSignedTransaction(keyPair, {
        to: keyPair.fingerprint,
        amount: staking.stakedAmount,
        nonce,
        parents,
        kind: "unstake",
        gasPrice: 0.001,
      });

      const res = await fetch(`${NODE_URL}/tx`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(signedTx),
      });

      const data = await res.json();
      if (res.ok && data.hash) {
        setResult(`Unstaked ${staking.stakedAmount} RKU (tx: ${data.hash.slice(0, 12)}...)`);
        setTimeout(() => {
          fetchRewards();
          fetchStakingInfo();
        }, 1000);
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

  const getSerializedKey = (): string => {
    if (!keyPair) return "";
    return serializeKeyPair(keyPair);
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
        {!walletReady && (
          <div className="form-row">
            <input
              type="text"
              placeholder="wallet address (fingerprint)"
              value={address}
              onChange={(e) => setAddress(e.target.value)}
            />
            <button onClick={fetchRewards} disabled={!address || loading}>
              {loading ? "loading..." : "lookup"}
            </button>
          </div>
        )}

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
          <div className="wallet-section">
            <h4>wallet</h4>
            {!walletReady ? (
              <div className="wallet-setup">
                <button onClick={handleGenerateWallet} className="primary">
                  generate new wallet
                </button>
                <div className="stake-note">
                  or import an existing wallet from CLI
                </div>
                <div className="form-row">
                  <textarea
                    placeholder='paste full key JSON: {"publicKey":"...", "privateKey":"...", "fingerprint":"..."}'
                    value={keyInput}
                    onChange={(e) => setKeyInput(e.target.value)}
                    rows={3}
                    style={{ fontFamily: 'monospace', fontSize: '11px' }}
                  />
                </div>
                <button onClick={handleImportKey} className="secondary">
                  import key
                </button>
              </div>
            ) : (
              <div className="wallet-info">
                <div className="derived-address">
                  <span className="label">address:</span>
                  <span className="mono">{keyPair?.fingerprint}</span>
                </div>
                <div className="form-row">
                  <button onClick={() => setShowPrivateKey(!showPrivateKey)} className="secondary small">
                    {showPrivateKey ? 'hide' : 'show'} key
                  </button>
                  <button onClick={() => {
                    navigator.clipboard.writeText(getSerializedKey());
                    setResult('Key copied to clipboard!');
                  }} className="secondary small">
                    copy key
                  </button>
                  <button onClick={handleClearWallet} className="secondary small danger">
                    clear
                  </button>
                </div>
                {showPrivateKey && (
                  <div className="private-key-display">
                    <code>{getSerializedKey()}</code>
                  </div>
                )}
              </div>
            )}
          </div>

          {walletReady && (
            <div className="stake-actions">
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
                  disabled={stakeAmount <= 0}
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
          )}

          <div className="stake-note">
            transactions are signed locally - your private key never leaves your browser
          </div>
        </div>
      </div>

      {error && <div className="error">{error}</div>}
      {result && <div className="success">{result}</div>}
    </div>
  );
}
