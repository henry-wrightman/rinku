import { useState, useEffect } from "react";
import { useParams, Link } from "react-router-dom";
import { PageHeader } from "./components/PageHeader";
import { useRinku } from "./context/WalletContext";
import type { WeightProofResponse } from "./types";

interface TrustScoreData {
  loading: boolean;
  data?: WeightProofResponse;
  error?: string;
}

const getTrustScoreColor = (score: number): string => {
  if (score < 30) return "#bf616a";
  if (score < 70) return "#ebcb8b";
  return "#a3be8c";
};

const formatMicroRKU = (micro: number): string => {
  const rku = micro / 1_000_000;
  return rku.toLocaleString(undefined, { maximumFractionDigits: 0 });
};

interface TrustAndVoteSectionProps {
  trustData: TrustScoreData;
  hash: string;
  finalized: boolean;
  onVoteSubmitted: () => void;
}

const TrustAndVoteSection = ({
  trustData,
  hash,
  finalized,
  onVoteSubmitted,
}: TrustAndVoteSectionProps) => {
  const { wallet } = useRinku();
  const [voting, setVoting] = useState(false);
  const [voteResult, setVoteResult] = useState<{
    success: boolean;
    message: string;
  } | null>(null);

  const submitVote = async (voteType: "boost" | "suppress" | "neutral") => {
    if (!wallet) {
      setVoteResult({
        success: false,
        message: "Connect wallet to vote",
      });
      return;
    }
    setVoting(true);
    setVoteResult(null);
    try {
      const apiUrl = import.meta.env.VITE_API_URL;
      const res = await fetch(`${apiUrl}/api/tx/${hash}/vote`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          validator_pubkey: wallet.publicKey,
          vote: voteType,
        }),
      });
      const text = await res.text();
      let data;
      try {
        data = JSON.parse(text);
      } catch {
        setVoteResult({ success: false, message: "Invalid server response" });
        return;
      }
      if (res.ok && data.success) {
        setVoteResult({ success: true, message: `${voteType} vote submitted` });
        onVoteSubmitted();
      } else {
        setVoteResult({
          success: false,
          message: data.message || data.error || "Failed",
        });
      }
    } catch (err) {
      setVoteResult({
        success: false,
        message: err instanceof Error ? err.message : "Network error",
      });
    } finally {
      setVoting(false);
    }
  };

  const { data, loading, error } = trustData;
  const aw = data?.aggregated_weight as any;
  const attestationCount = aw?.attestation_count ?? aw?.attestationCount ?? 0;
  const hasAttestations = aw && attestationCount > 0;
  const boostStake = aw?.boost_stake_micro ?? aw?.boostStakeMicro ?? 0;
  const suppressStake = aw?.suppress_stake_micro ?? aw?.suppressStakeMicro ?? 0;
  const neutralStake = aw?.neutral_stake_micro ?? aw?.neutralStakeMicro ?? 0;
  const trust_score = data?.trust_score ?? 50;
  const boost_ratio = data?.boost_ratio ?? 0;
  const suppress_ratio = data?.suppress_ratio ?? 0;
  const neutralRatio = 100 - boost_ratio - suppress_ratio;
  const color = getTrustScoreColor(trust_score);

  return (
    <div
      className="trust-vote-section"
      style={{
        padding: "12px 16px",
        borderRadius: 0,
        marginTop: 16,
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 12,
          flexWrap: "wrap",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <span
            className="trust-label"
            style={{ fontSize: 12, fontWeight: 500 }}
          >
            trust
          </span>
          {loading ? (
            <span className="trust-muted" style={{ fontSize: 12 }}>
              ...
            </span>
          ) : error || !hasAttestations ? (
            <span className="trust-muted" style={{ fontSize: 12 }}>
              pending
            </span>
          ) : (
            <>
              <span style={{ fontSize: 20, fontWeight: 700, color }}>
                {trust_score}
              </span>
              <div
                style={{
                  width: 80,
                  height: 6,
                  borderRadius: 3,
                  overflow: "hidden",
                  display: "flex",
                }}
                className="trust-bar-bg"
              >
                {suppress_ratio > 0 && (
                  <div
                    style={{
                      width: `${suppress_ratio}%`,
                      background: "#bf616a",
                    }}
                  />
                )}
                {neutralRatio > 0 && (
                  <div
                    style={{ width: `${neutralRatio}%`, background: "#4c566a" }}
                  />
                )}
                {boost_ratio > 0 && (
                  <div
                    style={{ width: `${boost_ratio}%`, background: "#a3be8c" }}
                  />
                )}
              </div>
            </>
          )}
        </div>

        {hasAttestations && (
          <div
            style={{
              display: "flex",
              gap: 12,
              fontSize: 11,
              flexWrap: "wrap",
            }}
          >
            <span style={{ color: "#a3be8c" }}>
              +{formatMicroRKU(boostStake)}
            </span>
            <span className="trust-muted">~{formatMicroRKU(neutralStake)}</span>
            <span style={{ color: "#bf616a" }}>
              -{formatMicroRKU(suppressStake)}
            </span>
            {data?.checkpoint_height && (
              <span className="trust-muted">cp#{data.checkpoint_height}</span>
            )}
          </div>
        )}

        <div style={{ flex: 1 }} />

        {finalized ? (
          wallet ? (
            <div
              style={{
                display: "flex",
                alignItems: "baseline",
                gap: 6,
              }}
            >
              <button
                onClick={() => submitVote("boost")}
                disabled={voting}
                className="trust-btn trust-btn-boost"
                style={{
                  padding: "4px 10px",
                  borderRadius: 0,
                  fontSize: 11,
                  fontWeight: 500,
                  cursor: voting ? "not-allowed" : "pointer",
                }}
              >
                +
              </button>
              <button
                onClick={() => submitVote("neutral")}
                disabled={voting}
                className="trust-btn trust-btn-neutral"
                style={{
                  padding: "4px 10px",
                  borderRadius: 0,
                  fontSize: 11,
                  fontWeight: 500,
                  cursor: voting ? "not-allowed" : "pointer",
                }}
              >
                ~
              </button>
              <button
                onClick={() => submitVote("suppress")}
                disabled={voting}
                className="trust-btn trust-btn-suppress"
                style={{
                  padding: "4px 10px",
                  borderRadius: 0,
                  fontSize: 11,
                  fontWeight: 500,
                  cursor: voting ? "not-allowed" : "pointer",
                }}
              >
                -
              </button>
            </div>
          ) : (
            <span className="trust-muted" style={{ fontSize: 11 }}>
              connect wallet to vote
            </span>
          )
        ) : (
          <span className="trust-muted" style={{ fontSize: 11 }}>
            vote after finalization
          </span>
        )}
      </div>

      {voteResult && (
        <div
          style={{
            marginTop: 8,
            padding: "4px 8px",
            borderRadius: 4,
            fontSize: 11,
            background: voteResult.success
              ? "rgba(163, 190, 140, 0.15)"
              : "rgba(191, 97, 106, 0.15)",
            color: voteResult.success ? "#a3be8c" : "#bf616a",
          }}
        >
          {voteResult.message}
        </div>
      )}
    </div>
  );
};

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
  kind?: string;
  data?: string;
  memo?: string;
  references?: string[];
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
  const [trustScore, setTrustScore] = useState<TrustScoreData>({
    loading: false,
  });

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
          kind: txData.kind,
          data: txData.data,
          memo: txData.memo,
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

  // Fetch trust score / weight proof
  useEffect(() => {
    if (!hash) return;

    setTrustScore({ loading: true });
    fetch(`${NODE_URL}/api/tx/${hash}/weight-proof`)
      .then((res) => res.json())
      .then((data: WeightProofResponse) => {
        setTrustScore({ loading: false, data });
      })
      .catch(() => {
        setTrustScore({ loading: false, error: "Failed to fetch trust score" });
      });
  }, [hash]);

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
          {tx.kind && (
            <div className="meta-row">
              <span className="label">type</span>
              <span
                className="value"
                style={{
                  color:
                    tx.kind === "contract"
                      ? "#88c0d0"
                      : tx.kind === "stake"
                        ? "#a3be8c"
                        : tx.kind === "unstake"
                          ? "#ebcb8b"
                          : tx.kind === "claim_rewards"
                            ? "#b48ead"
                            : "#d8dee9",
                }}
              >
                {tx.kind === "claim_rewards" ? "claim rewards" : tx.kind}
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
          {tx.amount === 0 && (tx.memo || tx.references) && !tx.finalized && (
            <div className="meta-row">
              <span className="label">fast-path</span>
              <span className="value" style={{ color: "#88c0d0" }}>
                eligible (~200ms finality)
              </span>
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

        {/* Trust Score & Vote Section */}
        <TrustAndVoteSection
          trustData={trustScore}
          hash={hash || ""}
          finalized={tx.finalized || !!tx.finality}
          onVoteSubmitted={() => {
            setTimeout(() => {
              fetch(`${NODE_URL}/api/tx/${hash}/weight-proof`)
                .then((res) => res.json())
                .then((data: WeightProofResponse) =>
                  setTrustScore({ loading: false, data }),
                )
                .catch(() => {});
            }, 1000);
          }}
        />

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
