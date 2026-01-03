import { useState, useEffect } from "react";
import { useParams, Link } from "react-router-dom";

interface TransactionNode {
  hash: string;
  from: string;
  to: string;
  amount: number;
  fee: number;
  nonce: number;
  ts: number;
  tipUrls: string[];
  sig: string;
  url: string;
  weight: number;
  finality?: {
    checkpointId: string;
    checkpointHeight: number;
    finalizedAt: number;
  };
}

interface PrunedInfo {
  pruned: true;
  checkpointId: string;
  checkpointHeight: number;
  prunedAt: number;
}

function HashTransactionPage() {
  const { hash } = useParams<{ hash: string }>();
  const [tx, setTx] = useState<TransactionNode | null>(null);
  const [prunedInfo, setPrunedInfo] = useState<PrunedInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!hash) {
      setError("No transaction hash");
      setLoading(false);
      return;
    }

    fetch(`/api/tx/${hash}`)
      .then(async (res) => {
        const data = await res.json();
        if (res.status === 410 && data.pruned) {
          setPrunedInfo(data as PrunedInfo);
          return null;
        }
        if (!res.ok) throw new Error(data.error || "Transaction not found");
        return data;
      })
      .then((data) => {
        if (!data) return;
        const txData = data.tx || data;
        setTx({
          hash: txData.hash,
          from: txData.from,
          to: txData.to,
          amount: txData.amount,
          fee: txData.fee ?? 0,
          nonce: txData.nonce,
          ts: txData.ts,
          tipUrls: txData.tipUrls || data.parentUrls || [],
          sig: txData.sig,
          url: data.url || `/tx/h/${txData.hash}`,
          weight: data.weight ?? txData.weight ?? 0,
          finality: data.finality || txData.finality,
        });
      })
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  }, [hash]);

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

  if (prunedInfo) {
    return (
      <div className="container">
        <header>
          <Link to="/" style={{ textDecoration: "none", color: "inherit" }}>
            <h1>rinku explorer</h1>
          </Link>
          <p>url-native distributed ledger</p>
        </header>
        <div className="section tx-detail">
          <div className="pruned-notice">
            <h2>transaction pruned</h2>
            <p>
              This transaction was pruned from active memory after being
              finalized. It is cryptographically verified and included in the
              ledger.
            </p>
            <div className="tx-meta">
              <div className="meta-row">
                <span className="label">hash</span>
                <span className="value mono">{truncate(hash || "", 24)}</span>
              </div>
              <div className="meta-row">
                <span className="label">status</span>
                <span className="value" style={{ color: "#a3be8c" }}>
                  finalized & pruned
                </span>
              </div>
              <div className="meta-row">
                <span className="label">checkpoint</span>
                <span className="value mono">
                  {truncate(prunedInfo.checkpointId, 16)}
                </span>
              </div>
              <div className="meta-row">
                <span className="label">checkpoint height</span>
                <span className="value">{prunedInfo.checkpointHeight}</span>
              </div>
              <div className="meta-row">
                <span className="label">pruned at</span>
                <span className="value">{formatTime(prunedInfo.prunedAt)}</span>
              </div>
            </div>
            <div className="tx-note" style={{ marginTop: 16 }}>
              <p>
                Pruned transactions are still part of the permanent ledger. The
                checkpoint contains a Merkle root that cryptographically proves
                this transaction existed.
              </p>
            </div>
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
          <h2>transaction</h2>
          <button className="btn-small" onClick={copyUrl}>
            {copied ? "copied!" : "copy url"}
          </button>
        </div>

        <div className="tx-amount">
          {tx.amount.toLocaleString()} <span className="unit">RKU</span>
          {tx.fee > 0 && (
            <span
              className="fee"
              style={{ color: "#ebcb8b", marginLeft: 8, fontSize: "0.7em" }}
            >
              (+{tx.fee} fee)
            </span>
          )}
        </div>

        <div className="tx-flow">
          <div className="address from">
            <span className="label">from</span>
            <Link
              to={
                tx.from === "genesis" || tx.from === "faucet"
                  ? "#"
                  : `/account/${tx.from}`
              }
              className="value"
              style={{ textDecoration: "none", color: "inherit" }}
            >
              {tx.from === "genesis" || tx.from === "faucet"
                ? tx.from
                : truncate(tx.from, 20)}
            </Link>
          </div>
          <span className="arrow">→</span>
          <div className="address to">
            <span className="label">to</span>
            <Link
              to={`/account/${tx.to}`}
              className="value"
              style={{ textDecoration: "none", color: "inherit" }}
            >
              {truncate(tx.to, 20)}
            </Link>
          </div>
        </div>

        <div className="tx-meta">
          <div className="meta-row">
            <span className="label">hash</span>
            <span className="value mono">{truncate(tx.hash, 24)}</span>
          </div>
          <div className="meta-row">
            <span className="label">timestamp</span>
            <span className="value">{formatTime(tx.ts)}</span>
          </div>
          <div className="meta-row">
            <span className="label">nonce</span>
            <span className="value">{tx.nonce}</span>
          </div>
          <div className="meta-row">
            <span className="label">gas fee</span>
            <span
              className="value"
              style={{ color: tx.fee > 0 ? "#ebcb8b" : undefined }}
            >
              {tx.fee}
            </span>
          </div>
          <div className="meta-row">
            <span className="label">weight</span>
            <span className="value">{(tx.weight ?? 0).toFixed(2)}</span>
          </div>
          <div className="meta-row">
            <span className="label">signature</span>
            <span className="value mono">{truncate(tx.sig, 24)}</span>
          </div>
          {tx.finality && (
            <>
              <div className="meta-row">
                <span className="label">status</span>
                <span className="value" style={{ color: "#a3be8c" }}>
                  finalized
                </span>
              </div>
              <div className="meta-row">
                <span className="label">checkpoint</span>
                <span className="value mono">
                  {truncate(tx.finality.checkpointId, 16)}
                </span>
              </div>
            </>
          )}
        </div>

        <div className="tx-parents">
          <h3>parent transactions ({tx.tipUrls?.length || 0})</h3>
          {!tx.tipUrls || tx.tipUrls.length === 0 ? (
            <div className="empty">genesis block - no parents</div>
          ) : (
            <div className="parent-list">
              {tx.tipUrls.map((parentUrl, i) => (
                <Link key={i} to={parentUrl} className="parent-link">
                  <span className="index">#{i + 1}</span>
                  <span className="parent-url">{truncate(parentUrl, 50)}</span>
                </Link>
              ))}
            </div>
          )}
        </div>

        <div className="tx-note">
          <p>
            this transaction is stored on the dag and can be verified by
            checking its hash, signature, and parent references.
          </p>
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

export default HashTransactionPage;
