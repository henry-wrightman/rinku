import { useState, useEffect } from "react";
import { useParams, Link } from "react-router-dom";
import { PageHeader } from "./components/PageHeader";
import type { TransactionKind, WalletChain, WalletChainEntry } from "@rinku/core";

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

interface ChainResponse {
  chain: WalletChain;
  isComplete: boolean;
  entryCount: number;
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
  
  const [walletChain, setWalletChain] = useState<WalletChain | null>(null);
  const [chainLoading, setChainLoading] = useState(false);
  const [chainError, setChainError] = useState<string | null>(null);
  const [isChainComplete, setIsChainComplete] = useState(false);
  const [showChainPanel, setShowChainPanel] = useState(false);

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
    };

    fetchAccountData();
    const interval = setInterval(fetchAccountData, 5000);
    return () => clearInterval(interval);
  }, [address]);

  const formatTime = (ts: number) => {
    return new Date(ts).toLocaleString();
  };

  const truncate = (s: string, len = 16) => {
    if (!s || s.length <= len) return s;
    return `${s.slice(0, len)}...`;
  };

  const formatTxKind = (kind?: TransactionKind): { label: string; color: string } => {
    switch (kind) {
      case 'stake': return { label: 'stake', color: '#a3be8c' };
      case 'unstake': return { label: 'unstake', color: '#ebcb8b' };
      case 'claim_rewards': return { label: 'claim', color: '#b48ead' };
      case 'contract': return { label: 'contract', color: '#88c0d0' };
      case 'consolidation': return { label: 'consolidate', color: '#81a1c1' };
      case 'reward': return { label: 'reward', color: '#b48ead' };
      default: return { label: 'transfer', color: '#d8dee9' };
    }
  };

  const copyAddress = () => {
    if (address) {
      navigator.clipboard.writeText(address);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  };

  const crawlWalletChain = async () => {
    if (!address) return;
    
    setChainLoading(true);
    setChainError(null);
    setShowChainPanel(true);
    
    try {
      const res = await fetch(`/api/account/${address}/chain?limit=100`);
      if (!res.ok) {
        const text = await res.text();
        throw new Error(text || 'Failed to fetch wallet chain');
      }
      const data: ChainResponse = await res.json();
      setWalletChain(data.chain);
      setIsChainComplete(data.isComplete);
    } catch (e: any) {
      setChainError(e.message || 'Failed to crawl wallet chain');
    } finally {
      setChainLoading(false);
    }
  };

  const loadMoreChain = async () => {
    if (!address || !walletChain?.entries.length) return;
    
    const lastEntry = walletChain.entries[walletChain.entries.length - 1];
    if (!lastEntry.prevTx) return;
    
    setChainLoading(true);
    try {
      const res = await fetch(`/api/account/${address}/chain?limit=100&from_tx=${lastEntry.prevTx}`);
      if (!res.ok) throw new Error('Failed to load more');
      const data: ChainResponse = await res.json();
      setWalletChain({
        ...walletChain,
        entries: [...walletChain.entries, ...data.chain.entries],
      });
      setIsChainComplete(data.isComplete);
    } catch (e: any) {
      setChainError(e.message);
    } finally {
      setChainLoading(false);
    }
  };

  const exportChainJson = () => {
    if (!walletChain) return;
    const blob = new Blob([JSON.stringify(walletChain, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `wallet-chain-${address?.slice(0, 8)}.json`;
    a.click();
    URL.revokeObjectURL(url);
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
                <span className="value">{rewards.witnessRewards.toFixed(2)}</span>
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

        <div style={{ marginTop: 24, marginBottom: 16 }}>
          <button 
            className="btn-small" 
            onClick={crawlWalletChain}
            disabled={chainLoading}
            style={{ 
              background: '#2e3440', 
              border: '1px solid #4c566a',
              padding: '8px 16px',
              cursor: chainLoading ? 'wait' : 'pointer'
            }}
          >
            {chainLoading ? 'crawling...' : 'crawl wallet chain'}
          </button>
          <span style={{ marginLeft: 12, color: '#616e88', fontSize: 12 }}>
            reconstruct full history from distributed protocol
          </span>
        </div>

        {showChainPanel && (
          <div style={{ 
            marginBottom: 24, 
            padding: 16, 
            background: '#1a1d23', 
            border: '1px solid #3b4252',
            borderRadius: 4
          }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 12 }}>
              <h3 style={{ margin: 0, color: '#88c0d0' }}>
                wallet chain {walletChain ? `(${walletChain.entries.length} entries)` : ''}
              </h3>
              <div>
                {walletChain && (
                  <button 
                    className="btn-small" 
                    onClick={exportChainJson}
                    style={{ marginRight: 8, fontSize: 11 }}
                  >
                    export JSON
                  </button>
                )}
                <button 
                  className="btn-small" 
                  onClick={() => setShowChainPanel(false)}
                  style={{ fontSize: 11 }}
                >
                  close
                </button>
              </div>
            </div>

            {chainError && (
              <div style={{ color: '#bf616a', marginBottom: 12 }}>{chainError}</div>
            )}

            {chainLoading && !walletChain && (
              <div style={{ color: '#616e88' }}>crawling chain from node...</div>
            )}

            {walletChain && (
              <>
                <div style={{ marginBottom: 12, fontSize: 12, color: '#616e88' }}>
                  <span>exported at: {new Date(walletChain.exportedAt).toLocaleString()}</span>
                  {walletChain.exportedBy && (
                    <span style={{ marginLeft: 12 }}>by: {truncate(walletChain.exportedBy, 24)}</span>
                  )}
                  <span style={{ marginLeft: 12, color: isChainComplete ? '#a3be8c' : '#ebcb8b' }}>
                    {isChainComplete ? '✓ complete' : '⋯ partial'}
                  </span>
                </div>

                <div style={{ maxHeight: 300, overflowY: 'auto' }}>
                  {walletChain.entries.map((entry, i) => (
                    <div 
                      key={entry.hash} 
                      style={{ 
                        padding: '8px 12px', 
                        borderBottom: '1px solid #2e3440',
                        display: 'flex',
                        justifyContent: 'space-between',
                        alignItems: 'center',
                        fontSize: 13
                      }}
                    >
                      <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
                        <span style={{ color: '#4c566a', fontSize: 11, width: 24 }}>#{entry.nonce}</span>
                        <Link to={`/tx/${entry.hash}`} style={{ color: '#81a1c1', fontFamily: 'monospace' }}>
                          {truncate(entry.hash, 12)}
                        </Link>
                        <span style={{ color: '#616e88' }}>→</span>
                        <span style={{ color: '#d8dee9' }}>{truncate(entry.to, 10)}</span>
                      </div>
                      <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
                        <span style={{ color: '#a3be8c' }}>{entry.amount} RKU</span>
                        {entry.proofUrl && (
                          <span style={{ color: '#88c0d0', fontSize: 10 }} title={entry.proofUrl}>✓ proof</span>
                        )}
                        {entry.checkpointHeight && (
                          <span style={{ color: '#b48ead', fontSize: 10 }}>cp:{entry.checkpointHeight}</span>
                        )}
                      </div>
                    </div>
                  ))}
                </div>

                {!isChainComplete && walletChain.entries.length > 0 && (
                  <button 
                    className="btn-small" 
                    onClick={loadMoreChain}
                    disabled={chainLoading}
                    style={{ marginTop: 12, width: '100%' }}
                  >
                    {chainLoading ? 'loading...' : 'load more'}
                  </button>
                )}
              </>
            )}
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
                const txType = formatTxKind(tx.kind);
                const isSpecialTx = tx.kind && tx.kind !== 'transfer';
                return (
                  <Link
                    key={i}
                    to={tx.url || `/tx/${tx.hash}`}
                    className="parent-link"
                  >
                    <span
                      className="index"
                      style={{ color: isSpecialTx ? txType.color : (isIncoming ? "#a3be8c" : "#bf616a") }}
                    >
                      {isSpecialTx ? txType.label.charAt(0).toUpperCase() : (isIncoming ? "+" : "-")}
                    </span>
                    <span className="parent-info">
                      {isSpecialTx ? (
                        <span style={{ color: txType.color }}>{txType.label}</span>
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
                      style={{ color: isSpecialTx ? txType.color : (isIncoming ? "#a3be8c" : "#bf616a") }}
                    >
                      {isSpecialTx ? "" : (isIncoming ? "+" : "-")}
                      {tx.amount > 0 ? `${tx.amount} RKU` : (isSpecialTx ? "" : "0 RKU")}
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
