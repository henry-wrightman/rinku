import { useState, useEffect } from "react";
import { useParams, Link } from "react-router-dom";
import { PageHeader } from "./components/PageHeader";
const getApiBaseUrl = () => {
  const envApiUrl = import.meta.env.VITE_API_URL;

  // If VITE_API_URL is set and not localhost, use it directly
  if (
    envApiUrl &&
    !envApiUrl.includes("127.0.0.1") &&
    !envApiUrl.includes("localhost")
  ) {
    console.log("Using VITE_API_URL htx:", envApiUrl);
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

    fetch(`${NODE_URL}/api/tx/${hash}`)
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

  useEffect(() => {
    if (!hash || (!tx?.finalized && !tx?.finality)) return;

    setProofLoading(true);
    fetch(`${NODE_URL}/api/tx/${hash}/proof`)
      .then((res) => res.json())
      .then((data) => setProofData(data))
      .catch(() =>
        setProofData({
          txHash: hash,
          finalized: false,
          error: "Failed to fetch proof",
        }),
      )
      .finally(() => setProofLoading(false));
  }, [hash, tx?.finalized, tx?.finality]);

  const fetchProof = async () => {
    if (!hash) return;
    setProofLoading(true);
    try {
      const res = await fetch(`${NODE_URL}/api/tx/${hash}/proof`);
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
          <div
            className="tx-proof"
            style={{
              marginTop: 24,
              padding: 20,
              background: "rgba(136, 192, 208, 0.1)",
              borderRadius: 0,
              border: "1px solid rgba(136, 192, 208, 0.3)",
              marginBottom: 20,
            }}
          >
            <h3 style={{ margin: "0 0 12px 0", color: "#88c0d0" }}>
              self-provable url
            </h3>

            {proofLoading ? (
              <div style={{ color: "#d8dee9", opacity: 0.7 }}>
                loading proof...
              </div>
            ) : proofData?.proofUrl ? (
              <>
                <div className="proof-url-box">{proofData.proofUrl}</div>
                <div className="proof-actions">
                  <button
                    className={`btn-proof ${proofCopied ? "btn-proof-success" : ""}`}
                    onClick={copyProof}
                  >
                    {proofCopied ? "copied!" : "copy proof url"}
                  </button>
                  <Link
                    to={{ pathname: "/", search: "?tab=verify" }}
                    className="btn-proof btn-proof-verify"
                  >
                    verify
                  </Link>
                  <span className="proof-meta">
                    {proofData.proofSizeBytes?.toLocaleString()} bytes
                  </span>
                </div>
                <p className="proof-description">
                  this proof is completely self-contained. anyone can verify
                  this transaction offline using only the url above.
                </p>
              </>
            ) : proofData?.error ? (
              <div style={{ color: "#bf616a", opacity: 0.8 }}>
                {proofData.error}
              </div>
            ) : (
              <div style={{ color: "#ebcb8b", opacity: 0.8 }}>
                awaiting finalization...
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
