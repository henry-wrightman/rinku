import { useState } from "react";
import { Link } from "react-router-dom";
import type { DAGNode } from "../types";
import { truncate, timeAgo } from "../utils";
import { Pagination } from "./Pagination";

interface DAGTabProps {
  nodes: DAGNode[];
  merkleRoot: string;
}

const PAGE_SIZE = 20;

export function DAGTab({ nodes, merkleRoot }: DAGTabProps) {
  const [page, setPage] = useState(0);

  if (nodes.length === 0) {
    return <div className="empty">no transactions yet</div>;
  }

  const reversedNodes = nodes.slice().reverse();
  const pageNodes = reversedNodes.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE);

  return (
    <div className="section">
      {pageNodes.map((node) => (
        <div key={node.tx.hash} className="dag-node">
          <div className="hash">{truncate(node.tx.hash, 12)}</div>
          <div className="amount">{node.tx.amount.toLocaleString()} coins</div>
          <div className="meta">
            {node.tx.from === "genesis" ? "genesis" : truncate(node.tx.from, 6)} →{" "}
            {truncate(node.tx.to, 6)} · {timeAgo(node.tx.ts)} · refs{" "}
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
