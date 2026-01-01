import React, { useState, useEffect } from "react";
import { useParams, Link } from "react-router-dom";
import { parseTransactionURL } from "@rinku/core";

interface TransactionData {
  from: string;
  to: string;
  amount: number;
  nonce: number;
  tipUrls: string[];
  sig: string;
  ts: number;
  hash?: string;
}

interface ApiResponse {
  status: string;
  message?: string;
  tx?: TransactionData;
  url?: string;
}

function TransactionPage() {
  const { payload } = useParams<{ payload: string }>();
  const [tx, setTx] = useState<TransactionData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!payload) {
      setError("No transaction payload");
      setLoading(false);
      return;
    }

    const parsed = parseTransactionURL(`/tx/${payload}`);
    if (parsed) {
      setTx(parsed);
      setLoading(false);
    } else {
      fetch(`/api/tx/resolve/${payload}`)
        .then((res) => res.json())
        .then((data: ApiResponse) => {
          if (data.tx) {
            setTx(data.tx);
          } else {
            setError("Could not parse transaction");
          }
        })
        .catch(() => setError("Failed to load transaction"))
        .finally(() => setLoading(false));
    }
  }, [payload]);

  const formatTime = (ts: number) => {
    return new Date(ts).toLocaleString();
  };

  const truncate = (s: string, len = 16) => {
    if (!s || s.length <= len) return s;
    return `${s.slice(0, len)}...`;
  };

  const copyUrl = () => {
    const fullUrl = window.location.href;
    navigator.clipboard.writeText(fullUrl);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
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
        <div className="loading">loading transaction...</div>
      </div>
    );
  }

  if (error || !tx) {
    return (
      <div className="container">
        <header>
          <Link to="/" style={{ textDecoration: "none", color: "inherit" }}>
            <h1>rinku explorer</h1>
          </Link>
          <p>url-native distributed ledger</p>
        </header>
        <div className="section">
          <div className="error">{error || "Transaction not found"}</div>
          <Link to="/" className="link" style={{ marginTop: 20, display: "block" }}>
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
          <h2>transaction</h2>
          <button className="btn-small" onClick={copyUrl}>
            {copied ? "copied!" : "copy url"}
          </button>
        </div>

        <div className="tx-amount">
          {tx.amount.toLocaleString()} <span className="unit">coins</span>
        </div>

        <div className="tx-flow">
          <div className="address from">
            <span className="label">from</span>
            <span className="value">{tx.from === "genesis" || tx.from === "faucet" ? tx.from : truncate(tx.from, 20)}</span>
          </div>
          <span className="arrow">→</span>
          <div className="address to">
            <span className="label">to</span>
            <span className="value">{truncate(tx.to, 20)}</span>
          </div>
        </div>

        <div className="tx-meta">
          <div className="meta-row">
            <span className="label">timestamp</span>
            <span className="value">{formatTime(tx.ts)}</span>
          </div>
          <div className="meta-row">
            <span className="label">nonce</span>
            <span className="value">{tx.nonce}</span>
          </div>
          {tx.hash && (
            <div className="meta-row">
              <span className="label">hash</span>
              <span className="value mono">{truncate(tx.hash, 24)}</span>
            </div>
          )}
          <div className="meta-row">
            <span className="label">signature</span>
            <span className="value mono">{truncate(tx.sig, 24)}</span>
          </div>
        </div>

        <div className="tx-parents">
          <h3>parent transactions ({tx.tipUrls.length})</h3>
          {tx.tipUrls.length === 0 ? (
            <div className="empty">genesis block - no parents</div>
          ) : (
            <div className="parent-list">
              {tx.tipUrls.map((parentUrl, i) => {
                const parentTx = parseTransactionURL(parentUrl);
                return (
                  <Link
                    key={i}
                    to={parentUrl}
                    className="parent-link"
                  >
                    <span className="index">#{i + 1}</span>
                    {parentTx ? (
                      <span className="parent-info">
                        {parentTx.from === "genesis" || parentTx.from === "faucet" 
                          ? parentTx.from 
                          : truncate(parentTx.from, 8)} → {truncate(parentTx.to, 8)}
                        <span className="parent-amount">{parentTx.amount} coins</span>
                      </span>
                    ) : (
                      <span className="parent-url">{truncate(parentUrl, 40)}</span>
                    )}
                  </Link>
                );
              })}
            </div>
          )}
        </div>

        <div className="tx-note">
          <p>
            this transaction is self-contained in the url. anyone can validate it
            by decoding the payload and verifying the signature and parent references.
          </p>
        </div>

        <Link to="/" className="link" style={{ marginTop: 20, display: "block" }}>
          ← back to explorer
        </Link>
      </div>
    </div>
  );
}

export default TransactionPage;
