import React, { useState, useEffect } from "react";
import { useParams, Link } from "react-router-dom";
import { parseTransactionURL, TransactionKind } from "@rinku/core";
import { PageHeader } from "./components/PageHeader";
import type { WeightProofResponse } from "./types";

const getApiBaseUrl = () => {
  const envApiUrl = import.meta.env.VITE_API_URL;

  // If VITE_API_URL is set and not localhost, use it directly
  if (
    envApiUrl &&
    !envApiUrl.includes("127.0.0.1") &&
    !envApiUrl.includes("localhost")
  ) {
    console.log("Using VITE_API_URL tx:", envApiUrl);
    return `${envApiUrl}`;
  }

  if (import.meta.env.PROD) {
    // Production: transform port in hostname
    const host = window.location.hostname;
    console.log("prod api url", `https://${host.replace(/-5000\./, "-3001.")}`);
    return `https://${host.replace(/-5000\./, "-3001.")}`;
  }
  return ""; // Dev: use Vite proxy (fetch calls already include /api prefix)
};
const NODE_URL = getApiBaseUrl();

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
  kind?: TransactionKind;
  data?: string;
  effectiveAmount?: number;
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

interface TrustScoreData {
  loading: boolean;
  data?: WeightProofResponse;
  error?: string;
}

interface FastPathStatusData {
  hash: string;
  status: string;
  aggregated_stake: number;
  quorum_threshold: number;
  quorum_percent: number;
  ack_count: number;
  finality_time_ms?: number;
}

const getTrustScoreColor = (score: number): string => {
  if (score < 30) return "#bf616a";
  if (score < 70) return "#ebcb8b";
  return "#a3be8c";
};

const formatMicroRKU = (micro: number): string => {
  const rku = micro / 1_000_000;
  return rku.toLocaleString(undefined, { maximumFractionDigits: 2 });
};

const TrustScoreSection = ({ data, loading, error }: TrustScoreData) => {
  if (loading) {
    return (
      <div
        className="trust-section"
        style={{
          marginTop: 24,
          padding: 16,
          background: "rgba(136, 192, 208, 0.05)",
          borderRadius: 0,
          border: "1px solid rgba(136, 192, 208, 0.2)",
        }}
      >
        <h3 style={{ margin: "0 0 12px 0", color: "#88c0d0" }}>trust score</h3>
        <div style={{ color: "#d8dee9", opacity: 0.7 }}>
          loading attestations...
        </div>
      </div>
    );
  }

  if (error || !data) {
    return (
      <div
        className="trust-section"
        style={{
          marginTop: 24,
          padding: 16,
          background: "rgba(136, 192, 208, 0.05)",
          borderRadius: 0,
          border: "1px solid rgba(136, 192, 208, 0.2)",
        }}
      >
        <h3 style={{ margin: "0 0 12px 0", color: "#88c0d0" }}>trust score</h3>
        <div style={{ color: "#ebcb8b", opacity: 0.8 }}>
          {data?.aggregated_weight.attestation_count === 0
            ? "no attestations yet"
            : error || "failed to load"}
        </div>
      </div>
    );
  }

  const {
    aggregated_weight,
    trust_score,
    boost_ratio,
    suppress_ratio,
    merkle_proof,
    checkpoint_height,
  } = data;
  const color = getTrustScoreColor(trust_score);
  const neutralRatio = 100 - boost_ratio - suppress_ratio;

  if (aggregated_weight.attestation_count === 0) {
    return (
      <div
        className="trust-section"
        style={{
          marginTop: 24,
          padding: 16,
          background: "rgba(136, 192, 208, 0.05)",
          borderRadius: 0,
          border: "1px solid rgba(136, 192, 208, 0.2)",
        }}
      >
        <h3 style={{ margin: "0 0 12px 0", color: "#88c0d0" }}>trust score</h3>
        <div style={{ color: "#ebcb8b", opacity: 0.8 }}>
          no attestations yet
        </div>
      </div>
    );
  }

  return (
    <div
      className="trust-section"
      style={{
        marginTop: 24,
        padding: 16,
        background: "rgba(136, 192, 208, 0.05)",
        borderRadius: 0,
        border: "1px solid rgba(136, 192, 208, 0.2)",
      }}
    >
      <h3 style={{ margin: "0 0 16px 0", color: "#88c0d0" }}>trust score</h3>

      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 16,
          marginBottom: 16,
        }}
      >
        <div
          style={{
            fontSize: 36,
            fontWeight: "bold",
            color,
          }}
        >
          {trust_score}
        </div>
        <div style={{ flex: 1 }}>
          <div
            style={{
              height: 8,
              background: "#2e3440",
              borderRadius: 4,
              overflow: "hidden",
              display: "flex",
            }}
          >
            {suppress_ratio > 0 && (
              <div
                style={{
                  width: `${suppress_ratio}%`,
                  background: "#bf616a",
                  height: "100%",
                }}
              />
            )}
            {neutralRatio > 0 && (
              <div
                style={{
                  width: `${neutralRatio}%`,
                  background: "#4c566a",
                  height: "100%",
                }}
              />
            )}
            {boost_ratio > 0 && (
              <div
                style={{
                  width: `${boost_ratio}%`,
                  background: "#a3be8c",
                  height: "100%",
                }}
              />
            )}
          </div>
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              marginTop: 4,
              fontSize: 11,
              color: "#d8dee9",
              opacity: 0.7,
            }}
          >
            <span>suppressed</span>
            <span>neutral</span>
            <span>boosted</span>
          </div>
        </div>
      </div>

      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(3, 1fr)",
          gap: 12,
          marginBottom: 16,
        }}
      >
        <div
          style={{
            padding: 12,
            background: "rgba(163, 190, 140, 0.1)",
            borderRadius: 4,
          }}
        >
          <div style={{ fontSize: 11, color: "#a3be8c", marginBottom: 4 }}>
            boost stake
          </div>
          <div style={{ fontSize: 14, color: "#d8dee9" }}>
            {formatMicroRKU(aggregated_weight.boost_stake_micro)} RKU
          </div>
          <div style={{ fontSize: 11, color: "#d8dee9", opacity: 0.6 }}>
            {boost_ratio.toFixed(1)}%
          </div>
        </div>
        <div
          style={{
            padding: 12,
            background: "rgba(76, 86, 106, 0.3)",
            borderRadius: 4,
          }}
        >
          <div style={{ fontSize: 11, color: "#81a1c1", marginBottom: 4 }}>
            neutral stake
          </div>
          <div style={{ fontSize: 14, color: "#d8dee9" }}>
            {formatMicroRKU(aggregated_weight.neutral_stake_micro)} RKU
          </div>
          <div style={{ fontSize: 11, color: "#d8dee9", opacity: 0.6 }}>
            {neutralRatio.toFixed(1)}%
          </div>
        </div>
        <div
          style={{
            padding: 12,
            background: "rgba(191, 97, 106, 0.1)",
            borderRadius: 4,
          }}
        >
          <div style={{ fontSize: 11, color: "#bf616a", marginBottom: 4 }}>
            suppress stake
          </div>
          <div style={{ fontSize: 14, color: "#d8dee9" }}>
            {formatMicroRKU(aggregated_weight.suppress_stake_micro)} RKU
          </div>
          <div style={{ fontSize: 11, color: "#d8dee9", opacity: 0.6 }}>
            {suppress_ratio.toFixed(1)}%
          </div>
        </div>
      </div>

      <div
        style={{
          display: "flex",
          gap: 16,
          fontSize: 12,
          color: "#d8dee9",
          opacity: 0.7,
        }}
      >
        <span>
          {aggregated_weight.attestation_count} attestation
          {aggregated_weight.attestation_count !== 1 ? "s" : ""}
        </span>
        {checkpoint_height && <span>checkpoint #{checkpoint_height}</span>}
        {merkle_proof.length > 0 && (
          <span style={{ color: "#a3be8c" }}>
            proof available ({merkle_proof.length} hashes)
          </span>
        )}
      </div>
    </div>
  );
};

function TransactionPage() {
  const { payload } = useParams<{ payload: string }>();
  const [tx, setTx] = useState<TransactionData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [proofCopied, setProofCopied] = useState(false);
  const [proof, setProof] = useState<ProofData>({ loading: false });
  const [trustScore, setTrustScore] = useState<TrustScoreData>({
    loading: false,
  });
  const [fpStatus, setFpStatus] = useState<FastPathStatusData | null>(null);

  useEffect(() => {
    if (!payload) {
      setError("No transaction payload");
      setLoading(false);
      return;
    }

    const parsed = parseTransactionURL(`${NODE_URL}/tx/${payload}`);
    if (parsed) {
      setTx(parsed);
      setLoading(false);
    } else {
      fetch(`${NODE_URL}/api/tx/resolve/${payload}`)
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
    fetch(`${NODE_URL}/api/tx/${tx.hash}/proof`)
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

    // Fetch trust score / weight proof
    setTrustScore({ loading: true });
    fetch(`${NODE_URL}/api/tx/${tx.hash}/weight-proof`)
      .then((res) => res.json())
      .then((data: WeightProofResponse) => {
        setTrustScore({ loading: false, data });
      })
      .catch(() => {
        setTrustScore({ loading: false, error: "Failed to fetch trust score" });
      });

    // Fetch fast-path finality status
    fetch(`${NODE_URL}/api/tx/fast/${tx.hash}`)
      .then((res) => res.json())
      .then((data: FastPathStatusData) => {
        setFpStatus(data);
      })
      .catch(() => {});
  }, [tx?.hash]);

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
        return { label: "claim rewards", color: "#b48ead" };
      case "contract":
        return { label: "contract call", color: "#88c0d0" };
      case "consolidation":
        return { label: "consolidation", color: "#81a1c1" };
      case "reward":
        return { label: "reward", color: "#b48ead" };
      default:
        return { label: "transfer", color: "#d8dee9" };
    }
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
          {(tx.effectiveAmount != null && tx.effectiveAmount > 0)
            ? tx.effectiveAmount.toLocaleString()
            : tx.amount.toLocaleString()}{" "}
          <span className="unit">RKU</span>
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
            <span className="label">type</span>
            <span
              className="value"
              style={{ color: formatTxKind(tx.kind).color }}
            >
              {formatTxKind(tx.kind).label}
            </span>
          </div>
          {fpStatus && (
            <div className="meta-row">
              <span className="label">status</span>
              <span
                className="value"
                style={{
                  color:
                    fpStatus.status === "confirmed" ||
                    fpStatus.status === "executed" ||
                    fpStatus.status === "finalized"
                      ? "#a3be8c"
                      : "#ebcb8b",
                  fontWeight: "bold",
                }}
              >
                {fpStatus.status === "confirmed" ||
                fpStatus.status === "executed" ||
                fpStatus.status === "finalized"
                  ? "finalized"
                  : fpStatus.status}
                {fpStatus.finality_time_ms != null &&
                  ` (${fpStatus.finality_time_ms}ms)`}
                {fpStatus.quorum_percent > 0 &&
                  fpStatus.status !== "confirmed" &&
                  fpStatus.status !== "executed" &&
                  fpStatus.status !== "finalized" &&
                  ` · ${fpStatus.quorum_percent}% quorum`}
              </span>
            </div>
          )}
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
            <span
              className="value mono"
              style={{ opacity: tx.sig && tx.sig !== "sig" ? 1 : 0.5 }}
            >
              {tx.sig && tx.sig !== "sig"
                ? truncate(tx.sig, 24)
                : "(system tx)"}
            </span>
          </div>
        </div>

        {tx.data &&
          (() => {
            let parsed: any = null;
            try {
              parsed = JSON.parse(tx.data);
            } catch {}
            return (
              <div
                style={{
                  marginTop: 16,
                  padding: 16,
                  background: "rgba(136, 192, 208, 0.05)",
                  border: "1px solid rgba(136, 192, 208, 0.2)",
                  borderRadius: 0,
                }}
              >
                <h3
                  style={{
                    margin: "0 0 12px 0",
                    color: "#88c0d0",
                    fontSize: 14,
                  }}
                >
                  transaction data
                </h3>
                {parsed ? (
                  <div
                    style={{ display: "flex", flexDirection: "column", gap: 8 }}
                  >
                    {parsed.action && (
                      <div className="meta-row">
                        <span className="label">action</span>
                        <span className="value" style={{ color: "#88c0d0" }}>
                          {parsed.action}
                        </span>
                      </div>
                    )}
                    {parsed.contractId && (
                      <div className="meta-row">
                        <span className="label">contract</span>
                        <Link
                          to={`/account/${parsed.contractId}`}
                          className="value mono"
                          style={{ color: "#88c0d0", textDecoration: "none" }}
                        >
                          {parsed.contractId}
                        </Link>
                      </div>
                    )}
                    {parsed.entrypoint && (
                      <div className="meta-row">
                        <span className="label">entrypoint</span>
                        <span
                          className="value mono"
                          style={{ color: "#a3be8c" }}
                        >
                          {parsed.entrypoint}()
                        </span>
                      </div>
                    )}
                    {parsed.input && (
                      <div style={{ marginTop: 4 }}>
                        <span
                          className="label"
                          style={{
                            display: "block",
                            marginBottom: 6,
                            fontSize: 12,
                            color: "#81a1c1",
                          }}
                        >
                          input
                        </span>
                        <pre
                          style={{
                            margin: 0,
                            padding: 12,
                            background: "rgba(46, 52, 64, 0.8)",
                            border: "1px solid rgba(76, 86, 106, 0.4)",
                            borderRadius: 0,
                            fontSize: 12,
                            color: "#d8dee9",
                            overflow: "auto",
                            maxHeight: 200,
                            whiteSpace: "pre-wrap",
                            wordBreak: "break-all",
                          }}
                        >
                          {JSON.stringify(parsed.input, null, 2)}
                        </pre>
                      </div>
                    )}
                  </div>
                ) : (
                  <pre
                    style={{
                      margin: 0,
                      padding: 12,
                      background: "rgba(46, 52, 64, 0.8)",
                      border: "1px solid rgba(76, 86, 106, 0.4)",
                      borderRadius: 0,
                      fontSize: 12,
                      color: "#d8dee9",
                      overflow: "auto",
                      maxHeight: 200,
                      whiteSpace: "pre-wrap",
                      wordBreak: "break-all",
                    }}
                  >
                    {tx.data}
                  </pre>
                )}
              </div>
            );
          })()}

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

        {tx.hash && (
          <TrustScoreSection
            loading={trustScore.loading}
            data={trustScore.data}
            error={trustScore.error}
          />
        )}

        {tx.hash ? (
          <div
            className="tx-proof"
            style={{
              marginTop: 24,
              padding: 16,
              background: "rgba(136, 192, 208, 0.1)",
              borderRadius: 0,
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
                <div className="proof-url-box">{proof.proofUrl}</div>
                <div className="proof-actions">
                  <button
                    className={`btn-proof ${proofCopied ? "btn-proof-success" : ""}`}
                    onClick={copyProofUrl}
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
                    {proof.sizeBytes?.toLocaleString()} bytes
                  </span>
                </div>
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
