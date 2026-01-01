import { useState, useEffect, useRef } from "react";
import { Link } from "react-router-dom";
import type { DAGNode } from "../types";
import { truncate, timeAgo } from "../utils";
import { Pagination } from "./Pagination";

interface DAGTabProps {
  nodes: DAGNode[];
  merkleRoot: string;
}

const PAGE_SIZE = 20;
const NEW_TX_DURATION = 4000;

export function DAGTab({ nodes, merkleRoot }: DAGTabProps) {
  const [page, setPage] = useState(0);
  const [newHashes, setNewHashes] = useState<Set<string>>(new Set());
  const prevHashesRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    const currentHashes = new Set(nodes.map((n) => n.tx.hash));
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

  if (nodes.length === 0) {
    return <div className="empty">no transactions yet</div>;
  }

  const reversedNodes = nodes.slice().reverse();
  const pageNodes = reversedNodes.slice(
    page * PAGE_SIZE,
    (page + 1) * PAGE_SIZE,
  );

  return (
    <div className="section">
      {pageNodes.map((node) => (
        <div
          key={node.tx.hash}
          className={`dag-node ${newHashes.has(node.tx.hash) ? "new-tx" : ""}`}
        >
          <div className="hash">
            {/* {newHashes.has(node.tx.hash) && (
              <span className="new-badge">NEW</span>
            )} */}
            <span className={newHashes.has(node.tx.hash) ? "typewriter" : ""}>
              {truncate(node.tx.hash, 12)}
            </span>
          </div>
          <div className="amount">{node.tx.amount.toLocaleString()} coins</div>
          <div className="meta">
            {node.tx.from === "genesis" ? "genesis" : truncate(node.tx.from, 6)}{" "}
            → {truncate(node.tx.to, 6)} · {timeAgo(node.tx.ts)} · refs{" "}
            {(node.tx.tipUrls || []).length} parent(s)
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
            <Link to={node.url} className="link">
              view
            </Link>
          </div>
        </div>
      ))}

      <Pagination
        page={page}
        totalItems={nodes.length}
        pageSize={PAGE_SIZE}
        onPageChange={setPage}
      />

      {merkleRoot && (
        <div style={{ marginTop: 20, color: "#555", fontSize: 12 }}>
          merkle root: {truncate(merkleRoot, 16)}
        </div>
      )}
    </div>
  );
}
