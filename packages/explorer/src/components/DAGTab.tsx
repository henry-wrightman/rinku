import { useState, useEffect, useRef, useCallback } from "react";
import { Link } from "react-router-dom";
import type { DAGNode, TransactionKind } from "../types";
import { truncate, timeAgo } from "../utils";
import { Pagination } from "./Pagination";

const getApiBaseUrl = () => {
  const envApiUrl = import.meta.env.VITE_API_URL;

  // If VITE_API_URL is set and not localhost, use it directly
  if (
    envApiUrl &&
    !envApiUrl.includes("127.0.0.1") &&
    !envApiUrl.includes("localhost")
  ) {
    console.log("Using VITE_API_URL:", envApiUrl);
    return `${envApiUrl}/api`;
  }
  return "/api";
};
const NODE_URL = getApiBaseUrl();

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
    case "relay":
      return { label: "relay", color: "#5e81ac" };
    default:
      return { label: "transfer", color: "#d8dee9" };
  }
};

const getTrustScoreColor = (score: number): string => {
  if (score < 30) return "#bf616a";
  if (score < 70) return "#ebcb8b";
  return "#a3be8c";
};

const TrustScoreBadge = ({
  score,
  count,
}: {
  score?: number;
  count?: number;
}) => {
  if (score === undefined || count === undefined || count === 0) {
    return null;
  }

  const color = getTrustScoreColor(score);

  return (
    <span
      className="trust-badge"
      style={{
        position: "absolute",
        top: 8,
        right: 8,
        display: "inline-flex",
        alignItems: "center",
        gap: 4,
        padding: "3px 8px",
        borderRadius: 4,
        backgroundColor: `${color}22`,
        border: `1px solid ${color}44`,
        fontSize: 12,
        fontWeight: 500,
        color,
      }}
      title={`Trust score: ${score}/100 (${count} attestation${count !== 1 ? "s" : ""})`}
    >
      <span style={{ fontSize: 11 }}>
        {score >= 70 ? "+" : score < 30 ? "-" : "~"}
      </span>
      {score}
    </span>
  );
};

interface ProofState {
  loading: boolean;
  copied: boolean;
  error?: string;
}

interface DAGTabProps {
  nodes: DAGNode[];
  merkleRoot: string;
  page: number;
  totalNodes: number;
  hasMore: boolean;
  onPageChange: (page: number) => void;
}

const PAGE_SIZE = 20;
const NEW_TX_DURATION = 3000;

export function DAGTab({
  nodes,
  merkleRoot,
  page,
  totalNodes,
  hasMore,
  onPageChange,
}: DAGTabProps) {
  const [newHashes, setNewHashes] = useState<Set<string>>(new Set());
  const [proofStates, setProofStates] = useState<Record<string, ProofState>>(
    {},
  );
  const prevHashesRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    const currentHashes = new Set(nodes.map((n) => n.hash));
    const freshHashes = new Set<string>();

    currentHashes.forEach((hash) => {
      if (!prevHashesRef.current.has(hash)) {
        freshHashes.add(hash);
      }
    });

    if (freshHashes.size > 0 && prevHashesRef.current.size > 0) {
      setNewHashes(freshHashes);
      setTimeout(() => setNewHashes(new Set()), NEW_TX_DURATION);
    }

    prevHashesRef.current = currentHashes;
  }, [nodes]);

  const copyProofUrl = useCallback(async (hash: string) => {
    setProofStates((prev) => ({
      ...prev,
      [hash]: { loading: true, copied: false },
    }));

    try {
      const apiUrl = NODE_URL.endsWith("/api") ? NODE_URL : `${NODE_URL}/api`;
      const res = await fetch(`${apiUrl}/tx/${hash}/proof`);
      const data = await res.json();

      if (data.proofUrl) {
        await navigator.clipboard.writeText(data.proofUrl);
        setProofStates((prev) => ({
          ...prev,
          [hash]: { loading: false, copied: true },
        }));
        setTimeout(() => {
          setProofStates((prev) => ({
            ...prev,
            [hash]: { loading: false, copied: false },
          }));
        }, 2000);
      } else {
        setProofStates((prev) => ({
          ...prev,
          [hash]: {
            loading: false,
            copied: false,
            error: data.error || "Not finalized",
          },
        }));
        setTimeout(() => {
          setProofStates((prev) => ({
            ...prev,
            [hash]: { loading: false, copied: false },
          }));
        }, 2000);
      }
    } catch {
      setProofStates((prev) => ({
        ...prev,
        [hash]: { loading: false, copied: false, error: "Failed" },
      }));
    }
  }, []);

  if (nodes.length === 0) {
    return <div className="empty">no transactions yet</div>;
  }

  return (
    <div className="section">
      {nodes.map((node) => (
        <div
          key={node.hash}
          className={`dag-node ${newHashes.has(node.hash) ? "new-tx" : ""}`}
          style={{ position: "relative" }}
        >
          <TrustScoreBadge
            score={node.trust_score}
            count={node.attestation_count}
          />
          <div className="hash">
            <span className={newHashes.has(node.hash) ? "typewriter" : ""}>
              {truncate(node.hash, 12)}
            </span>
          </div>
          <div className="amount">
            {node.amount.toLocaleString()} RKU
            {node.fee > 0 && (
              <span className="fee"> (+{node.fee?.toFixed(5)} fee)</span>
            )}
          </div>
          <div className="meta">
            <span
              className="tx-kind-label"
              style={{ color: formatTxKind(node.kind).color }}
            >
              {formatTxKind(node.kind).label}
            </span>
            {" · "}
            {node.kind === "relay"
              ? <><span style={{ color: "#5e81ac" }}>relayer</span>{" → "}{truncate(node.to, 6)}</>
              : <>{node.from === "genesis" ? "genesis" : truncate(node.from, 6)}{" → "}{truncate(node.to, 6)}</>} ·{" "}
            {timeAgo(node.ts)} · refs {node.parentCount} parent(s) ·{" "}
            <span
              style={{
                color:
                  node.fast_path_status === "confirmed" ||
                  node.fast_path_status === "executed" ||
                  node.fast_path_status === "finalized"
                    ? "#a3be8c"
                    : node.finalized
                      ? "#a3be8c"
                      : "#ebcb8b",
              }}
            >
              {node.fast_path_status === "confirmed" ||
              node.fast_path_status === "executed" ||
              node.fast_path_status === "finalized"
                ? `confirmed${node.fast_path_finality_ms ? ` (${node.fast_path_finality_ms}ms)` : ""}${node.finalized ? " + finalized" : ""}`
                : node.finalized
                  ? "finalized"
                  : "pending"}
            </span>
          </div>
          <div className="actions">
            {/* <span
              className="link"
              onClick={() => {
                const fullUrl = window.location.origin + node.url;
                navigator.clipboard.writeText(fullUrl);
              }}
            >
              copy url
            </span> */}
            <Link to={node.url} className="link">
              view
            </Link>{" "}
            {(node.fast_path_status === "confirmed" ||
              node.fast_path_status === "executed" ||
              node.fast_path_status === "finalized") && (
              <Link to={`?tab=thread&hash=${node.hash}`} className="rlink">
                reply
              </Link>
            )}
            {(node.fast_path_status === "confirmed" ||
              node.fast_path_status === "executed" ||
              node.fast_path_status === "finalized") && (
              <Link
                to={`/tx/h/${node.hash}#vote`}
                className="link"
                style={{ marginLeft: 8 }}
              >
                vote
              </Link>
            )}
            {node.finalized && (
              <>
                {" · "}
                <span
                  className="link"
                  style={{
                    color: proofStates[node.hash]?.copied
                      ? "#a3be8c"
                      : proofStates[node.hash]?.error
                        ? "#bf616a"
                        : "#88c0d0",
                  }}
                  onClick={() => copyProofUrl(node.hash)}
                >
                  {proofStates[node.hash]?.loading
                    ? "..."
                    : proofStates[node.hash]?.copied
                      ? "proof copied!"
                      : proofStates[node.hash]?.error
                        ? proofStates[node.hash].error
                        : "copy proof"}
                </span>
              </>
            )}
          </div>
        </div>
      ))}

      <Pagination
        page={page}
        totalItems={totalNodes}
        pageSize={PAGE_SIZE}
        onPageChange={onPageChange}
      />

      {merkleRoot && (
        <div style={{ marginTop: 20, color: "#555", fontSize: 12 }}>
          merkle root: {truncate(merkleRoot, 16)}
        </div>
      )}
    </div>
  );
}
