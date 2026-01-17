import React, { useState, useEffect } from "react";
import { useParams, Link } from "react-router-dom";
import { parseTransactionURL } from "@rinku/core";
import { PageHeader } from "./components/PageHeader";

interface TransactionData {
  from: string;
  to: string;
  amount: number;
  fee?: number;
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

interface ProofData {
  loading: boolean;
  proofUrl?: string;
  error?: string;
  sizeBytes?: number;
  qrViable?: boolean;
}

function TransactionPage() {
  const { payload } = useParams<{ payload: string }>();
  const [tx, setTx] = useState<TransactionData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [proofCopied, setProofCopied] = useState(false);
  const [proof, setProof] = useState<ProofData>({ loading: false });

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

  useEffect(() => {
    if (!tx?.hash) return;

    setProof({ loading: true });
    fetch(`/api/tx/${tx.hash}/proof`)
      .then((res) => res.json())
      .then((data) => {
        if (data.proofUrl) {
          setProof({
            loading: false,
            proofUrl: data.proofUrl,
            sizeBytes: data.proofSizeBytes,
            qrViable: data.qrViable,
          });
        } else {
          setProof({
            loading: false,
            error: data.error || "Proof not available",
          });
        }
      })
      .catch(() => {
        setProof({ loading: false, error: "Failed to fetch proof" });
      });
  }, [tx?.hash]);

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

  const copyProofUrl = () => {
    if (proof.proofUrl) {
      navigator.clipboard.writeText(proof.proofUrl);
      setProofCopied(true);
      setTimeout(() => setProofCopied(false), 2000);
    }
  };

  if (loading) {
    return (
      <div className="container">
        <PageHeader />
        <div className="loading">loading transaction...</div>
      </div>
    );
  }

  if (error || !tx) {
    return (
      <div className="container">
        <PageHeader />
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
          {(tx.fee ?? 0) > 0 && (
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
            <span className="value">
              {tx.from === "genesis" || tx.from === "faucet"
                ? tx.from
                : truncate(tx.from, 20)}
            </span>
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
          <div className="meta-row">
            <span className="label">gas fee</span>
            <span
              className="value"
              style={{ color: (tx.fee ?? 0) > 0 ? "#ebcb8b" : undefined }}
            >
              {tx.fee ?? 0}
            </span>
          </div>
          {tx.hash && (
            <div className="meta-row">
              <span className="label">hash</span>
              <span className="value mono">{truncate(tx.hash, 24)}</span>
            </div>
          )}
          <div className="meta-row">
            <span className="label">signature</span>
            <span className="value mono" style={{ opacity: tx.sig ? 1 : 0.5 }}>
              {tx.sig ? truncate(tx.sig, 24) : "(system tx)"}
            </span>
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
                  <Link key={i} to={parentUrl} className="parent-link">
                    <span className="index">#{i + 1}</span>
                    {parentTx ? (
                      <span className="parent-info">
                        {parentTx.from === "genesis" ||
                        parentTx.from === "faucet"
                          ? parentTx.from
                          : truncate(parentTx.from, 8)}{" "}
                        → {truncate(parentTx.to, 8)}
                        <span className="parent-amount">
                          {parentTx.amount} RKU
                        </span>
                      </span>
                    ) : (
                      <span className="parent-url">
                        {truncate(parentUrl, 40)}
                      </span>
                    )}
                  </Link>
                );
              })}
            </div>
          )}
        </div>

        {tx.hash ? (
          <div
            className="tx-proof"
            style={{
              marginTop: 24,
              padding: 16,
              background: "rgba(136, 192, 208, 0.1)",
              borderRadius: 8,
              border: "1px solid rgba(136, 192, 208, 0.3)",
              marginBottom: 20,
            }}
          >
            <h3 style={{ margin: "0 0 12px 0", color: "#88c0d0" }}>
              self-provable url
            </h3>
            {proof.loading ? (
              <div style={{ color: "#d8dee9", opacity: 0.7 }}>
                loading proof...
              </div>
            ) : proof.proofUrl ? (
              <>
                <div
                  style={{
                    fontFamily: "monospace",
                    fontSize: 11,
                    wordBreak: "break-all",
                    background: "rgba(0,0,0,0.2)",
                    padding: 12,
                    borderRadius: 4,
                    marginBottom: 12,
                    maxHeight: 100,
                    overflow: "auto",
                  }}
                >
                  {proof.proofUrl}
                </div>
                <div
                  style={{
                    display: "flex",
                    gap: 12,
                    alignItems: "center",
                    flexWrap: "wrap",
                  }}
                >
                  <button
                    className="btn-small"
                    onClick={copyProofUrl}
                    style={{ background: proofCopied ? "#a3be8c" : "#88c0d0" }}
                  >
                    {proofCopied ? "copied!" : "copy proof url"}
                  </button>
                  <span
                    style={{ fontSize: 12, color: "#d8dee9", opacity: 0.7 }}
                  >
                    {proof.sizeBytes} bytes
                    {proof.qrViable && " · QR viable"}
                  </span>
                </div>
                <p
                  style={{
                    fontSize: 12,
                    marginTop: 12,
                    opacity: 0.8,
                    color: "#d8dee9",
                  }}
                >
                  this proof is completely self-contained. anyone can verify
                  this transaction offline using only the url above.
                </p>
              </>
            ) : (
              <div style={{ color: "#ebcb8b", opacity: 0.8 }}>
                {proof.error || "awaiting finalization..."}
              </div>
            )}
          </div>
        ) : (
          <div className="tx-note">
            <p>
              this transaction is self-contained in the url. anyone can validate
              it by decoding the payload and verifying the signature and parent
              references.
            </p>
            <p style={{ marginTop: 8, fontSize: "0.9em", opacity: 0.7 }}>
              once submitted to the network and finalized, a self-provable url
              will be available for offline verification.
            </p>
          </div>
        )}

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

export default TransactionPage;
