import { useState, useEffect, useRef, useCallback } from "react";
import { useParams, Link } from "react-router-dom";
import { useWebSocketContext } from "./context/WebSocketContext";
import { PageHeader } from "./components/PageHeader";
import type { TransactionKind } from "@rinku/core";

const getApiBaseUrl = () => {
  const envApiUrl = import.meta.env.VITE_API_URL;

  // If VITE_API_URL is set and not localhost, use it directly
  if (
    envApiUrl &&
    !envApiUrl.includes("127.0.0.1") &&
    !envApiUrl.includes("localhost")
  ) {
    console.log("Using VITE_API_URL:", envApiUrl);
    return `${envApiUrl}`;
  }

  if (import.meta.env.PROD) {
    // Production on Replit: transform port in hostname
    const host = window.location.hostname;
    console.log(
      "prod api url (Replit)",
      `https://${host.replace(/-5000\./, "-3001.")}`,
    );
    return `https://${host.replace(/-5000\./, "-3001.")}`;
  }
  return ""; // Dev: use Vite proxy (fetch calls already include /api prefix)
};
const NODE_URL = getApiBaseUrl();

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
  kind?: TransactionKind;
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

interface StateProofData {
  success: boolean;
  proofUrl?: string;
  verified?: boolean;
  error?: string;
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
  const [stateProof, setStateProof] = useState<StateProofData | null>(null);
  const [proofLoading, setProofLoading] = useState(false);
  const [proofCopied, setProofCopied] = useState(false);

  const fetchAccountData = useCallback(async () => {
    if (!address) {
      setError("No address provided");
      setLoading(false);
      return;
    }
    try {
      const [accountRes, dagRes, stakingRes, rewardsRes] = await Promise.all([
        fetch(`${NODE_URL}/api/account/${address}`),
        fetch(`${NODE_URL}/api/dag`),
        fetch(`${NODE_URL}/api/staking/${address}`),
        fetch(`${NODE_URL}/api/rewards/${address}`),
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
          kind: node.kind,
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
  }, [address]);

  useEffect(() => {
    fetchAccountData();
  }, [fetchAccountData]);

  const { status: wsStatus, lastBatch } = useWebSocketContext();
  const lastBatchIdRef = useRef(0);
  const acctRefreshRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (!lastBatch || lastBatch.id === lastBatchIdRef.current) return;
    lastBatchIdRef.current = lastBatch.id;
    let shouldRefresh = false;
    for (const evt of lastBatch.items) {
      if (evt.type === 'AccountUpdated') {
        const data = evt.data as { address: string };
        if (data.address === address) {
          shouldRefresh = true;
          break;
        }
      } else if (evt.type === 'FastPathExecuted' || evt.type === 'CheckpointCreated') {
        shouldRefresh = true;
        break;
      }
    }
    if (shouldRefresh && !acctRefreshRef.current) {
      acctRefreshRef.current = setTimeout(() => {
        acctRefreshRef.current = null;
        fetchAccountData();
      }, 250);
    }
  }, [lastBatch, address]);

  useEffect(() => {
    return () => {
      if (acctRefreshRef.current) {
        clearTimeout(acctRefreshRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (wsStatus === 'connected') return;
    const interval = setInterval(fetchAccountData, 5000);
    return () => clearInterval(interval);
  }, [wsStatus, address]);

  const formatTime = (ts: number) => {
    return new Date(ts).toLocaleString();
  };

  const truncate = (s: string, len = 16) => {
    if (!s || s.length <= len) return s;
    return `${s.slice(0, len)}...`;
  };

  const formatTxKind = (
    kind?: TransactionKind,
  ): { label: string; color: string } => {
    switch (kind) {
      case "stake":
        return { label: "stake", color: "#a3be8c" };
      case "unstake":
        return { label: "unstake", color: "#ebcb8b" };
      case "claim_rewards":
        return { label: "claim", color: "#b48ead" };
      case "contract":
        return { label: "contract", color: "#88c0d0" };
      case "consolidation":
        return { label: "consolidate", color: "#81a1c1" };
      case "reward":
        return { label: "reward", color: "#b48ead" };
      default:
        return { label: "transfer", color: "#d8dee9" };
    }
  };

  const copyAddress = () => {
    if (address) {
      navigator.clipboard.writeText(address);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  };

  const fetchStateProof = async () => {
    if (!address) return;
    setProofLoading(true);
    try {
      // Use on-demand endpoint to get fresh proof at current checkpoint
      const res = await fetch(`${NODE_URL}/api/account/${address}/proof/current`);
      const data = await res.json();
      setStateProof(data);
    } catch (e) {
      setStateProof({ success: false, error: "Failed to fetch state proof" });
    } finally {
      setProofLoading(false);
    }
  };

  const copyProofUrl = () => {
    if (stateProof?.proofUrl) {
      navigator.clipboard.writeText(stateProof.proofUrl);
      setProofCopied(true);
      setTimeout(() => setProofCopied(false), 2000);
    }
  };

  if (loading) {
    return (
      <div className="container">
        <PageHeader />
        <div className="loading">loading account...</div>
      </div>
    );
  }

  if (error || !account) {
    return (
      <div className="container">
        <PageHeader />
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
      <PageHeader />

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
                <span className="value">{rewards.tipRewards.toFixed(2)}</span>
              </div>
              <div className="meta-row">
                <span className="label">stake rewards</span>
                <span className="value">{rewards.stakeRewards.toFixed(2)}</span>
              </div>
              <div className="meta-row">
                <span className="label">witness rewards</span>
                <span className="value">
                  {rewards.witnessRewards.toFixed(2)}
                </span>
              </div>
              <div className="meta-row">
                <span className="label">total earned</span>
                <span className="value" style={{ color: "#b48ead" }}>
                  {rewards.totalRewards.toFixed(2)}
                </span>
              </div>
              {rewards.pendingRewards > 0 && (
                <div className="meta-row">
                  <span className="label">pending</span>
                  <span className="value" style={{ color: "#a3be8c" }}>
                    {rewards.pendingRewards.toFixed(2)}
                  </span>
                </div>
              )}
            </div>
          </div>
        )}

        <div style={{ marginTop: 24 }}>
          <h3 style={{ fontSize: 13, color: "#fff", marginBottom: 12 }}>
            state proof
          </h3>
          <p style={{ opacity: 0.6, marginBottom: 12, fontSize: 12 }}>
            Generate a self-contained proof of this account's balance that can be verified offline.
          </p>
          
          {!stateProof ? (
            <button
              className="btn-small"
              onClick={fetchStateProof}
              disabled={proofLoading}
              style={{ marginBottom: 8 }}
            >
              {proofLoading ? "generating..." : "get state proof"}
            </button>
          ) : stateProof.success && stateProof.proofUrl ? (
            <div className="tx-meta">
              <div className="meta-row">
                <span className="label">status</span>
                <span className="value" style={{ color: stateProof.verified ? "#22c55e" : "#ef4444" }}>
                  {stateProof.verified ? "✓ verified" : "✗ unverified"}
                </span>
              </div>
              <div className="meta-row" style={{ flexDirection: "column", alignItems: "flex-start", gap: 8 }}>
                <span className="label">proof url</span>
                <div style={{ display: "flex", gap: 8, width: "100%" }}>
                  <textarea
                    readOnly
                    value={stateProof.proofUrl}
                    rows={3}
                    style={{
                      flex: 1,
                      padding: "8px",
                      border: "1px solid #333",
                      borderRadius: "4px",
                      backgroundColor: "#1a1a1a",
                      color: "#88c0d0",
                      fontFamily: "monospace",
                      fontSize: "11px",
                      resize: "none",
                      wordBreak: "break-all",
                    }}
                  />
                </div>
                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <button className="btn-small" onClick={copyProofUrl}>
                    {proofCopied ? "copied!" : "copy url"}
                  </button>
                  <span style={{ opacity: 0.6, fontSize: 11 }}>
                    paste on verify tab to validate
                  </span>
                </div>
              </div>
            </div>
          ) : (
            <div style={{ color: "#ef4444", fontSize: 12 }}>
              {stateProof.error || "No proof available - account may not have finalized transactions yet."}
            </div>
          )}
        </div>

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
                const txType = formatTxKind(tx.kind);
                const isSpecialTx = tx.kind && tx.kind !== "transfer";
                return (
                  <Link
                    key={i}
                    to={tx.url || `/tx/${tx.hash}`}
                    className="parent-link"
                  >
                    <span
                      className="index"
                      style={{
                        color: isSpecialTx
                          ? txType.color
                          : isIncoming
                            ? "#a3be8c"
                            : "#bf616a",
                      }}
                    >
                      {isSpecialTx
                        ? txType.label.charAt(0).toUpperCase()
                        : isIncoming
                          ? "+"
                          : "-"}
                    </span>
                    <span className="parent-info">
                      {isSpecialTx ? (
                        <span style={{ color: txType.color }}>
                          {txType.label}
                        </span>
                      ) : isIncoming ? (
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
                      style={{
                        color: isSpecialTx
                          ? txType.color
                          : isIncoming
                            ? "#a3be8c"
                            : "#bf616a",
                      }}
                    >
                      {isSpecialTx ? "" : isIncoming ? "+" : "-"}
                      {tx.amount > 0
                        ? `${tx.amount} RKU`
                        : isSpecialTx
                          ? ""
                          : "0 RKU"}
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
