import { useState, useEffect, useRef, useCallback } from "react";
import { Link } from "react-router-dom";
import type { DAGNode, TransactionKind } from "../types";
import { truncate, timeAgo } from "../utils";
import { Pagination } from "./Pagination";

const getApiBaseUrl = () => {
  const envApiUrl = import.meta.env.VITE_API_URL;

  if (
    envApiUrl &&
    !envApiUrl.includes("127.0.0.1") &&
    !envApiUrl.includes("localhost")
  ) {
    return `${envApiUrl}/api`;
  }
  return "/api";
};
const NODE_URL = getApiBaseUrl();

const KIND_COLORS: Record<string, string> = {
  stake: "#a3be8c",
  unstake: "#ebcb8b",
  claim_rewards: "#b48ead",
  contract: "#88c0d0",
  consolidation: "#81a1c1",
  reward: "#b48ead",
  transfer: "#d8dee9",
};

const kindColor = (kind?: TransactionKind) =>
  KIND_COLORS[kind || "transfer"] || "#d8dee9";
const kindLabel = (kind?: TransactionKind) => {
  switch (kind) {
    case "stake":
      return "STK";
    case "unstake":
      return "USK";
    case "claim_rewards":
      return "CLM";
    case "contract":
      return "CTR";
    case "consolidation":
      return "CON";
    case "reward":
      return "RWD";
    default:
      return "TXF";
  }
};

const statusInfo = (node: DAGNode) => {
  const fp = node.fast_path_status;
  if (fp === "confirmed" || fp === "executed" || fp === "finalized") {
    const ms = node.fast_path_finality_ms
      ? `${node.fast_path_finality_ms}ms`
      : "";
    return {
      text: `finalized${ms ? ` ${ms}` : ""}`,
      color: "#a3be8c",
      cls: "st-confirmed",
      anchored: !!node.finalized,
    };
  }
  if (node.finalized)
    return { text: null, color: "#a3be8c", cls: "st-final", anchored: true };
  return { text: "pending", color: "#ebcb8b", cls: "st-pending", anchored: false };
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

const PAGE_SIZE = 50;
const NEW_TX_DURATION = 3000;
const MAX_VISIBLE_PER_GROUP = 5;

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
  const [expandedHash, setExpandedHash] = useState<string | null>(null);
  const [expandedGroups, setExpandedGroups] = useState<Set<string>>(new Set());
  const pinnedNodeRef = useRef<DAGNode | null>(null);
  const prevHashesRef = useRef<Set<string>>(new Set());

  const expandRow = useCallback((node: DAGNode | null) => {
    if (node) {
      pinnedNodeRef.current = node;
      setExpandedHash(node.hash);
    } else {
      pinnedNodeRef.current = null;
      setExpandedHash(null);
    }
  }, []);

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

  if (expandedHash) {
    const found = nodes.find((n) => n.hash === expandedHash);
    if (found) {
      pinnedNodeRef.current = found;
    }
  }

  const pinned = pinnedNodeRef.current;
  const hasPinnedInPage = pinned && nodes.some((n) => n.hash === pinned.hash);
  const effectiveNodes = pinned && !hasPinnedInPage ? [pinned, ...nodes] : nodes;

  if (effectiveNodes.length === 0) {
    return <div className="empty">no transactions yet</div>;
  }

  const GROUP_ORDER: (TransactionKind | "transfer")[] = [
    "transfer",
    "consolidation",
    "stake",
    "unstake",
    "claim_rewards",
    "contract",
    "reward",
  ];

  const GROUP_LABELS: Record<string, string> = {
    transfer: "transfers",
    consolidation: "consolidations",
    stake: "stakes",
    unstake: "unstakes",
    claim_rewards: "claims",
    contract: "contracts",
    reward: "rewards",
  };

  const grouped = new Map<string, DAGNode[]>();
  for (const node of effectiveNodes) {
    const key = node.kind || "transfer";
    if (!grouped.has(key)) grouped.set(key, []);
    grouped.get(key)!.push(node);
  }

  const knownKeys = new Set<string>(GROUP_ORDER);
  const extraKeys = [...grouped.keys()].filter((k) => !knownKeys.has(k));

  const sortedGroups = [
    ...GROUP_ORDER.filter((k) => grouped.has(k)).map((k) => ({
      kind: k,
      label: GROUP_LABELS[k] || k,
      nodes: grouped.get(k)!,
      color: kindColor(k as TransactionKind),
    })),
    ...extraKeys.map((k) => ({
      kind: k,
      label: k,
      nodes: grouped.get(k)!,
      color: "#d8dee9",
    })),
  ].sort((a, b) => a.nodes.length - b.nodes.length);

  const renderRow = (node: DAGNode) => {
    const isNew = newHashes.has(node.hash);
    const isExpanded = expandedHash === node.hash;
    const isPinned = isExpanded && pinned?.hash === node.hash && !hasPinnedInPage;
    const st = statusInfo(node);
    const kc = kindColor(node.kind);
    const kl = kindLabel(node.kind);
    const isConfirmed = st.cls !== "st-pending";

    return (
      <div
        key={node.hash}
        className={`tx-row ${isNew ? "tx-new" : ""} ${isPinned ? "tx-pinned" : ""} ${st.cls}`}
        style={{ borderLeftColor: kc }}
        onClick={() => expandRow(isExpanded ? null : node)}
      >
        <div className="tx-row-main">
          <span className="tx-kind" style={{ color: kc }}>
            {kl}
          </span>
          <span className="tx-hash">{truncate(node.hash, 10)}</span>
          <span className="tx-amount">
            {node.amount > 0 ? `${node.amount.toLocaleString()} RKU` : ""}
          </span>
          {node.fee > 0 && (
            <span className="tx-fee">+{node.fee.toFixed(5)}</span>
          )}
          <span className="tx-route">
            {node.from === "genesis" ? "genesis" : truncate(node.from, 5)}
            <span className="arrow">{"\u2192"}</span>
            {truncate(node.to, 5)}
          </span>
          <span className="tx-time">{timeAgo(node.ts)}</span>
          {st.text && <span className={`tx-status ${st.cls}`}>{st.text}</span>}
          {st.anchored && <span className="tx-status st-anchored">anchored</span>}
          {node.trust_score !== undefined &&
            node.attestation_count !== undefined &&
            node.attestation_count > 0 && (
              <span
                className="tx-trust"
                style={{
                  color:
                    node.trust_score >= 70
                      ? "#a3be8c"
                      : node.trust_score < 30
                        ? "#bf616a"
                        : "#ebcb8b",
                }}
                title={`Trust: ${node.trust_score}/100 (${node.attestation_count} votes)`}
              >
                {node.trust_score >= 70
                  ? "+"
                  : node.trust_score < 30
                    ? "-"
                    : "~"}
                {node.trust_score}
              </span>
            )}
        </div>

        {isExpanded && (
          <div className="tx-expanded" onClick={(e) => e.stopPropagation()}>
            <div className="tx-detail-grid">
              <span className="td-label">hash</span>
              <span className="td-value mono">{node.hash}</span>
              <span className="td-label">from</span>
              <span className="td-value mono">{node.from}</span>
              <span className="td-label">to</span>
              <span className="td-value mono">{node.to}</span>
              <span className="td-label">parents</span>
              <span className="td-value">{node.parentCount}</span>
              {node.memo && (
                <>
                  <span className="td-label">memo</span>
                  <span className="td-value">{node.memo}</span>
                </>
              )}
            </div>
            <div className="tx-actions">
              <Link to={node.url} className="link">
                view
              </Link>
              {isConfirmed && (
                <Link to={`?tab=thread&hash=${node.hash}`} className="rlink">
                  reply
                </Link>
              )}
              {isConfirmed && (
                <Link to={`/tx/h/${node.hash}#vote`} className="link">
                  vote
                </Link>
              )}
              {node.finalized && (
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
              )}
            </div>
          </div>
        )}
      </div>
    );
  };

  const toggleGroup = (kind: string) => {
    setExpandedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(kind)) next.delete(kind);
      else next.add(kind);
      return next;
    });
  };

  return (
    <div className="dag-feed">
      {sortedGroups.map((g) => {
        const isGroupExpanded = expandedGroups.has(g.kind);
        const cap = isGroupExpanded ? g.nodes.length : MAX_VISIBLE_PER_GROUP;
        const visible = g.nodes.slice(0, cap);
        const pinnedInGroup = pinned && !hasPinnedInPage && (pinned.kind || "transfer") === g.kind
          && !visible.some((n) => n.hash === pinned.hash);
        if (pinnedInGroup) {
          visible.unshift(pinned);
        }
        const hidden = g.nodes.length - visible.length;

        return (
          <div className="feed-group" key={g.kind}>
            <div className="feed-group-header">
              <span className="feed-dot" style={{ background: g.color }} />
              {g.label} ({g.nodes.length})
              {g.nodes.length > MAX_VISIBLE_PER_GROUP && (
                <span
                  className="feed-group-toggle"
                  onClick={() => toggleGroup(g.kind)}
                >
                  {isGroupExpanded ? "collapse" : `+${hidden} more`}
                </span>
              )}
            </div>
            {visible.map(renderRow)}
          </div>
        );
      })}

      <Pagination
        page={page}
        totalItems={totalNodes}
        pageSize={PAGE_SIZE}
        onPageChange={onPageChange}
      />

      {merkleRoot && (
        <div style={{ marginTop: 16, color: "#444", fontSize: 11 }}>
          merkle root: {truncate(merkleRoot, 16)}
        </div>
      )}
    </div>
  );
}
