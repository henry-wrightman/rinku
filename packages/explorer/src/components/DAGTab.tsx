import { useState, useEffect, useRef, useCallback } from "react";
import { Link } from "react-router-dom";
import type { DAGNode } from "../types";
import { truncate, timeAgo } from "../utils";
import { Pagination } from "./Pagination";

interface ProofState {
  loading: boolean;
  copied: boolean;
  error?: string;
}

interface ExtendedDAGNode extends DAGNode {
  finalized?: boolean;
}

interface DAGTabProps {
  nodes: ExtendedDAGNode[];
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
  const [proofStates, setProofStates] = useState<Record<string, ProofState>>({});
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
    setProofStates(prev => ({ ...prev, [hash]: { loading: true, copied: false } }));
    
    try {
      const res = await fetch(`/api/tx/${hash}/proof`);
      const data = await res.json();
      
      if (data.proofUrl) {
        await navigator.clipboard.writeText(data.proofUrl);
        setProofStates(prev => ({ ...prev, [hash]: { loading: false, copied: true } }));
        setTimeout(() => {
          setProofStates(prev => ({ ...prev, [hash]: { loading: false, copied: false } }));
        }, 2000);
      } else {
        setProofStates(prev => ({ 
          ...prev, 
          [hash]: { loading: false, copied: false, error: data.error || "Not finalized" } 
        }));
        setTimeout(() => {
          setProofStates(prev => ({ ...prev, [hash]: { loading: false, copied: false } }));
        }, 2000);
      }
    } catch {
      setProofStates(prev => ({ 
        ...prev, 
        [hash]: { loading: false, copied: false, error: "Failed" } 
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
        >
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
            {node.from === "genesis" ? "genesis" : truncate(node.from, 6)} →{" "}
            {truncate(node.to, 6)} · {timeAgo(node.ts)} · refs{" "}
            {node.parentCount} parent(s) ·{" "}
            <span style={{ color: node.finalized ? "#a3be8c" : "#ebcb8b" }}>
              {node.finalized ? "finalized" : "pending"}
            </span>
          </div>
          <div className="actions">
            <span
              className="link"
              onClick={() => {
                const fullUrl = window.location.origin + node.url;
                navigator.clipboard.writeText(fullUrl);
              }}
            >
              copy url
            </span>
            {node.finalized && (
              <span
                className="link"
                style={{ color: proofStates[node.hash]?.copied ? "#a3be8c" : proofStates[node.hash]?.error ? "#bf616a" : "#88c0d0" }}
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
            <Link to={node.url} className="link">
              view
            </Link>
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
