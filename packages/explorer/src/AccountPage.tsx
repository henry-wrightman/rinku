import { useState, useEffect } from "react";
import { useParams, Link } from "react-router-dom";

interface AccountData {
  fingerprint: string;
  balance: number;
  nonce: number;
  firstTxTimestamp?: number;
}

interface Transaction {
  hash: string;
  from: string;
  to: string;
  amount: number;
  nonce: number;
  ts: number;
  url?: string;
}

interface StakingStatus {
  address: string;
  stakedAmount: number;
  isValidator: boolean;
  stakedAt: number | null;
  earnedRewards: number;
  canUnstakeAt: number | null;
}

interface RewardsSummary {
  address: string;
  tipRewards: number;
  stakeRewards: number;
  witnessRewards: number;
  totalRewards: number;
  pendingRewards: number;
}

function AccountPage() {
  const { address } = useParams<{ address: string }>();
  const [account, setAccount] = useState<AccountData | null>(null);
  const [transactions, setTransactions] = useState<Transaction[]>([]);
  const [staking, setStaking] = useState<StakingStatus | null>(null);
  const [rewards, setRewards] = useState<RewardsSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!address) {
      setError("No address provided");
      setLoading(false);
      return;
    }

    const fetchAccountData = async () => {
      try {
        const [accountRes, dagRes, stakingRes, rewardsRes] = await Promise.all([
          fetch(`/api/account/${address}`),
          fetch(`/api/dag`),
          fetch(`/api/staking/${address}`),
          fetch(`/api/rewards/${address}`),
        ]);

        if (!accountRes.ok) {
          setError("Account not found");
          setLoading(false);
          return;
        }

        const accountData = await accountRes.json();
        setAccount(accountData);

        const dagData = await dagRes.json();
        const accountTxs = (dagData.nodes || [])
          .filter(
            (node: any) =>
              node && (node.from === address || node.to === address),
          )
          .map((node: any) => ({
            hash: node.hash,
            from: node.from,
            to: node.to,
            amount: node.amount,
            nonce: node.nonce,
            ts: node.ts,
            url: node.url,
          }))
          .sort((a: Transaction, b: Transaction) => b.ts - a.ts);

        setTransactions(accountTxs);

        if (stakingRes.ok) {
          const stakingData = await stakingRes.json();
          setStaking(stakingData);
        }

        if (rewardsRes.ok) {
          const rewardsData = await rewardsRes.json();
          setRewards(rewardsData);
        }
      } catch (e) {
        console.error("Failed to fetch account:", e);
        setError("Failed to load account");
      } finally {
        setLoading(false);
      }
    };

    fetchAccountData();
  }, [address]);

  const formatTime = (ts: number) => {
    return new Date(ts).toLocaleString();
  };

  const truncate = (s: string, len = 16) => {
    if (!s || s.length <= len) return s;
    return `${s.slice(0, len)}...`;
  };

  const copyAddress = () => {
    if (address) {
      navigator.clipboard.writeText(address);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  };

  if (loading) {
    return (
      <div className="container">
        <header>
          <Link to="/" style={{ textDecoration: "none", color: "inherit" }}>
            <h1>rinku explorer</h1>
          </Link>
          <p>url-native distributed ledger</p>
        </header>
        <div className="loading">loading account...</div>
      </div>
    );
  }

  if (error || !account) {
    return (
      <div className="container">
        <header>
          <Link to="/" style={{ textDecoration: "none", color: "inherit" }}>
            <h1>rinku explorer</h1>
          </Link>
          <p>url-native distributed ledger</p>
        </header>
        <div className="section">
          <div className="error">{error || "Account not found"}</div>
          <Link
            to="/"
            className="link"
            style={{ marginTop: 20, display: "block" }}
          >
            ← back to explorer
          </Link>
        </div>
      </div>
    );
  }

  return (
    <div className="container">
      <header>
        <Link to="/" style={{ textDecoration: "none", color: "inherit" }}>
          <h1>rinku explorer</h1>
        </Link>
        <p>url-native distributed ledger</p>
      </header>

      <div className="section tx-detail">
        <div className="tx-header">
          <h2>account</h2>
          <button className="btn-small" onClick={copyAddress}>
            {copied ? "copied!" : "copy address"}
          </button>
        </div>

        <div className="tx-amount">
          {account.balance.toLocaleString()} <span className="unit">RKU</span>
        </div>

        <div className="tx-meta">
          <div className="meta-row">
            <span className="label">address</span>
            <span className="value mono">{address}</span>
          </div>
          <div className="meta-row">
            <span className="label">nonce</span>
            <span className="value">{account.nonce}</span>
          </div>
          {account.firstTxTimestamp && (
            <div className="meta-row">
              <span className="label">first seen</span>
              <span className="value">
                {formatTime(account.firstTxTimestamp)}
              </span>
            </div>
          )}
        </div>

        {staking && staking.stakedAmount > 0 && (
          <div style={{ marginTop: 24 }}>
            <h3 style={{ fontSize: 13, color: "#fff", marginBottom: 12 }}>
              staking
            </h3>
            <div className="tx-meta">
              <div className="meta-row">
                <span className="label">staked</span>
                <span className="value" style={{ color: "#a3be8c" }}>
                  {staking.stakedAmount} RKU
                </span>
              </div>
              <div className="meta-row">
                <span className="label">validator</span>
                <span
                  className="value"
                  style={{ color: staking.isValidator ? "#39ff14" : "#666" }}
                >
                  {staking.isValidator ? "active" : "no"}
                </span>
              </div>
              {staking.stakedAt && (
                <div className="meta-row">
                  <span className="label">staked since</span>
                  <span className="value">{formatTime(staking.stakedAt)}</span>
                </div>
              )}
            </div>
          </div>
        )}

        {rewards && rewards.totalRewards > 0 && (
          <div style={{ marginTop: 24 }}>
            <h3 style={{ fontSize: 13, color: "#fff", marginBottom: 12 }}>
              rewards
            </h3>
            <div className="tx-meta">
              <div className="meta-row">
                <span className="label">tip rewards</span>
                <span className="value">{rewards.tipRewards}</span>
              </div>
              <div className="meta-row">
                <span className="label">stake rewards</span>
                <span className="value">{rewards.stakeRewards}</span>
              </div>
              <div className="meta-row">
                <span className="label">witness rewards</span>
                <span className="value">{rewards.witnessRewards}</span>
              </div>
              <div className="meta-row">
                <span className="label">total earned</span>
                <span className="value" style={{ color: "#b48ead" }}>
                  {rewards.totalRewards}
                </span>
              </div>
              {rewards.pendingRewards > 0 && (
                <div className="meta-row">
                  <span className="label">pending</span>
                  <span className="value" style={{ color: "#a3be8c" }}>
                    {rewards.pendingRewards}
                  </span>
                </div>
              )}
            </div>
          </div>
        )}

        <div className="tx-parents" style={{ marginTop: 24 }}>
          <h3>
            transaction history ({transactions.length} indexed of{" "}
            {account.nonce} total)
          </h3>
          {transactions.length === 0 ? (
            <div className="empty">
              no transactions indexed (might've been pruned)
            </div>
          ) : (
            <div className="parent-list">
              {transactions.slice(0, 20).map((tx, i) => {
                const isIncoming = tx.to === address;
                return (
                  <Link
                    key={i}
                    to={tx.url || `/tx/${tx.hash}`}
                    className="parent-link"
                  >
                    <span
                      className="index"
                      style={{ color: isIncoming ? "#a3be8c" : "#bf616a" }}
                    >
                      {isIncoming ? "+" : "-"}
                    </span>
                    <span className="parent-info">
                      {isIncoming ? (
                        <>
                          from{" "}
                          {tx.from === "faucet"
                            ? "faucet"
                            : truncate(tx.from, 8)}
                        </>
                      ) : (
                        <>to {truncate(tx.to, 8)}</>
                      )}
                    </span>
                    <span
                      className="parent-amount"
                      style={{ color: isIncoming ? "#a3be8c" : "#bf616a" }}
                    >
                      {isIncoming ? "+" : "-"}
                      {tx.amount} RKU
                    </span>
                  </Link>
                );
              })}
              {transactions.length > 20 && (
                <div className="empty" style={{ marginTop: 12 }}>
                  showing 20 of {transactions.length} transactions
                </div>
              )}
            </div>
          )}
        </div>

        <Link
          to="/"
          className="link"
          style={{ marginTop: 20, display: "block" }}
        >
          ← back to explorer
        </Link>
      </div>
    </div>
  );
}

export default AccountPage;
