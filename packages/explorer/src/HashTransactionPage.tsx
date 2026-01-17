import { useState, useEffect } from "react";
import { useParams, Link } from "react-router-dom";
import { PageHeader } from "./components/PageHeader";

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
  finalized?: boolean;
  finality?: {
    checkpointId: string;
    checkpointHeight: number;
    finalizedAt: number;
  };
}

interface ProofResponse {
  txHash: string;
  finalized: boolean;
  proofUrl?: string;
  proofSizeBytes?: number;
  qrViable?: boolean;
  error?: string;
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
  const [proofLoading, setProofLoading] = useState(false);
  const [proofData, setProofData] = useState<ProofResponse | null>(null);
  const [proofCopied, setProofCopied] = useState(false);

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
          finalized: data.finalized ?? !!txData.finality,
          finality: data.finality || txData.finality,
        });
      })
      .catch((e) => setError(e.message))
      .finally(() => setLoading(false));
  }, [hash]);

  const fetchProof = async () => {
    if (!hash) return;
    setProofLoading(true);
    try {
      const res = await fetch(`/api/tx/${hash}/proof`);
      const data = await res.json();
      setProofData(data);
    } catch (e) {
      setProofData({
        txHash: hash,
        finalized: false,
        error: "Failed to fetch proof",
      });
    } finally {
      setProofLoading(false);
    }
  };

  const copyProof = () => {
    if (proofData?.proofUrl) {
      navigator.clipboard.writeText(proofData.proofUrl);
      setProofCopied(true);
      setTimeout(() => setProofCopied(false), 2000);
    }
  };

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
        <PageHeader />
        <div className="loading">loading transaction...</div>
      </div>
    );
  }

  if (prunedInfo) {
    return (
      <div className="container">
        <PageHeader />
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
              <p style={{ marginTop: 12 }}>
                If you have a self-contained proof URL for this transaction, you
                can verify it offline using the{" "}
                <Link
                  to={{ pathname: "/", search: "?tab=verify" }}
                  style={{ color: "#88c0d0" }}
                >
                  verify tab
                </Link>
                .
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
        <PageHeader />
        <div className="section">
          <div className="error">
            {error ||
              "Transaction not found (may have been pruned after finalization)"}
          </div>
          <p style={{ marginTop: 12, color: "#888", fontSize: "0.9em" }}>
            If this transaction was pruned, you can still verify it using a
            self-contained proof URL in the{" "}
            <Link
              to={{ pathname: "/", search: "?tab=verify" }}
              style={{ color: "#88c0d0" }}
            >
              verify tab
            </Link>
            .
          </p>
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
              (+{tx.fee?.toFixed(5)} fee)
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
            <span className="label">status</span>
            <span
              className="value"
              style={{ color: tx.finalized ? "#a3be8c" : "#ebcb8b" }}
            >
              {tx.finalized ? "finalized" : "pending"}
            </span>
          </div>
          <div className="meta-row">
            <span className="label">signature</span>
            <span className="value mono" style={{ opacity: tx.sig ? 1 : 0.5 }}>
              {tx.sig ? truncate(tx.sig, 24) : "(system tx)"}
            </span>
          </div>
          {tx.finality && (
            <>
              <div className="meta-row">
                <span className="label">checkpoint</span>
                <span className="value mono">
                  {truncate(tx.finality.checkpointId, 16)}
                </span>
              </div>
            </>
          )}
        </div>

        {(tx.finalized || tx.finality) && (
          <div className="tx-proof" style={{ marginTop: 16 }}>
            <h3>self-contained proof</h3>
            <p style={{ opacity: 0.7, fontSize: "0.85rem", marginBottom: 12 }}>
              Generate a cryptographic proof URL that can verify this
              transaction offline, without needing the network.
            </p>

            {!proofData && (
              <button
                className="btn-small"
                onClick={fetchProof}
                disabled={proofLoading}
                style={{ marginBottom: 12 }}
              >
                {proofLoading ? "generating..." : "generate proof"}
              </button>
            )}

            {proofData && proofData.proofUrl && (
              <div style={{ marginTop: 8 }}>
                <div
                  style={{
                    display: "flex",
                    gap: 8,
                    alignItems: "center",
                    marginBottom: 8,
                  }}
                >
                  <span style={{ color: "#a3be8c" }}>proof ready</span>
                  <button className="btn-small" onClick={copyProof}>
                    {proofCopied ? "copied!" : "copy proof url"}
                  </button>
                  <Link
                    to={{ pathname: "/", search: "?tab=verify" }}
                    className="btn-small"
                    style={{ textDecoration: "none" }}
                  >
                    verify
                  </Link>
                </div>
                <div
                  style={{
                    fontSize: "0.75rem",
                    opacity: 0.6,
                    marginBottom: 8,
                  }}
                >
                  size: {proofData.proofSizeBytes?.toLocaleString()} bytes
                  {proofData.qrViable
                    ? " (QR compatible)"
                    : " (too large for QR)"}
                </div>
                <textarea
                  readOnly
                  value={proofData.proofUrl}
                  rows={4}
                  style={{
                    width: "100%",
                    padding: "8px",
                    fontSize: "0.75rem",
                    fontFamily: "monospace",
                    backgroundColor: "var(--bg-secondary)",
                    border: "1px solid var(--border)",
                    borderRadius: 4,
                    color: "var(--text-primary)",
                    resize: "vertical",
                  }}
                />
              </div>
            )}

            {proofData && proofData.error && (
              <div style={{ color: "#bf616a", marginTop: 8 }}>
                {proofData.error}
              </div>
            )}

            {proofData && !proofData.proofUrl && !proofData.error && (
              <div style={{ opacity: 0.7, marginTop: 8 }}>
                Proof not available yet. Transaction may still be processing.
              </div>
            )}
          </div>
        )}

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
