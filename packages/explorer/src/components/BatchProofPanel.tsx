import { useState } from "react";
import { API_URL } from "../config";
import type { BatchProofData, ProofFreshness } from "../crypto";
import { verifyMerkleMultiProof } from "../crypto";

const NODE_URL = API_URL;

function normalizeFreshness(f: any): ProofFreshness | null {
  if (!f) return null;
  return {
    generatedAtCheckpoint: f.generatedAtCheckpoint ?? f.generated_at_checkpoint ?? 0,
    generatedAtTimestamp: f.generatedAtTimestamp ?? f.generated_at_timestamp ?? 0,
    chainTipAtGeneration: f.chainTipAtGeneration ?? f.chain_tip_at_generation ?? 0,
    maxAgeCheckpoints: f.maxAgeCheckpoints ?? f.max_age_checkpoints ?? null,
  };
}

function freshnessColor(age: number): string {
  if (age < 5) return "#22c55e";
  if (age <= 20) return "#f59e0b";
  return "#ef4444";
}

function freshnessLabel(age: number): string {
  if (age < 5) return "fresh";
  if (age <= 20) return "aging";
  return "stale";
}

interface BatchProofResponse {
  success: boolean;
  proof: BatchProofData | null;
  txCount: number;
  error?: string;
}

export function BatchProofPanel() {
  const [txHashesInput, setTxHashesInput] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [proof, setProof] = useState<BatchProofData | null>(null);
  const [verificationResult, setVerificationResult] = useState<{
    multiproofValid: boolean;
    verified: boolean;
  } | null>(null);
  const [chainTip, setChainTip] = useState<number | null>(null);

  const truncateHash = (hash: string) => {
    if (hash.length <= 20) return hash;
    return `${hash.slice(0, 10)}...${hash.slice(-10)}`;
  };

  const fetchBatchProof = async () => {
    const lines = txHashesInput
      .split(/[\n,]/)
      .map((s) => s.trim())
      .filter((s) => s.length > 0);

    if (lines.length === 0) {
      setError("Enter at least one transaction hash");
      return;
    }

    setLoading(true);
    setError(null);
    setProof(null);
    setVerificationResult(null);

    try {
      const res = await fetch(`${NODE_URL}/proof/batch`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ txHashes: lines, includeReceipts: false }),
      });

      const raw = await res.json();

      if (!raw.success || !raw.proof) {
        setError(raw.error || "Failed to generate batch proof");
        return;
      }

      const p = raw.proof;
      const mp = p.multiproof;
      const normalized: BatchProofData = {
        type: p.type,
        finality: {
          checkpointHeight: p.finality.checkpointHeight ?? p.finality.checkpoint_height,
          checkpointHash: p.finality.checkpointHash ?? p.finality.checkpoint_hash,
          checkpointTimestamp: p.finality.checkpointTimestamp ?? p.finality.checkpoint_timestamp,
          stateRoot: p.finality.stateRoot ?? p.finality.state_root,
          receiptRoot: p.finality.receiptRoot ?? p.finality.receipt_root ?? "",
          blsAggregatedSig: p.finality.blsAggregatedSig ?? p.finality.bls_aggregated_sig ?? null,
          blsSignerBitmap: p.finality.blsSignerBitmap ?? p.finality.bls_signer_bitmap ?? null,
        },
        txHashes: p.txHashes ?? p.tx_hashes ?? [],
        multiproof: {
          leafHashes: mp.leafHashes ?? mp.leaf_hashes ?? [],
          leafIndices: mp.leafIndices ?? mp.leaf_indices ?? [],
          helperHashes: mp.helperHashes ?? mp.helper_hashes ?? [],
          helperIndices: mp.helperIndices ?? mp.helper_indices ?? [],
          numLeaves: mp.numLeaves ?? mp.num_leaves ?? 0,
          root: mp.root,
        },
        receipts: p.receipts ?? null,
        chainId: p.chainId ?? p.chain_id ?? null,
        freshness: normalizeFreshness(p.freshness),
      };

      setProof(normalized);

      try {
        const tipRes = await fetch(`${NODE_URL}/chain/tip`);
        const tipData = await tipRes.json();
        setChainTip(tipData.checkpointHeight ?? tipData.checkpoint_height ?? null);
      } catch {}


      const multiproofValid = await verifyMerkleMultiProof(normalized.multiproof);
      setVerificationResult({
        multiproofValid,
        verified: true,
      });
    } catch (e: any) {
      setError(e.message || "Failed to fetch batch proof");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div style={{ marginTop: 16 }}>
      <div className="section">
        <h3>batch proof verification</h3>
        <p style={{ opacity: 0.7, marginBottom: "1rem", fontSize: "0.9rem" }}>
          Generate a batch proof for multiple transactions in the same
          checkpoint. Enter transaction hashes separated by newlines or commas.
        </p>

        <textarea
          value={txHashesInput}
          onChange={(e) => setTxHashesInput(e.target.value)}
          placeholder="Enter transaction hashes (one per line or comma-separated)"
          rows={4}
          style={{
            width: "100%",
            padding: "0.75rem",
            border: "1px solid var(--border)",
            borderRadius: "4px",
            backgroundColor: "var(--bg-secondary)",
            color: "var(--text-primary)",
            fontFamily: "monospace",
            fontSize: "0.85rem",
            resize: "vertical",
            boxSizing: "border-box",
            marginBottom: "0.5rem",
          }}
        />

        <button
          onClick={fetchBatchProof}
          disabled={loading}
          className="btn-proof btn-proof-verify"
        >
          {loading ? "generating..." : "generate batch proof"}
        </button>
      </div>

      {error && (
        <div
          className="section"
          style={{
            borderColor: "#ef4444",
            backgroundColor: "rgba(239, 68, 68, 0.1)",
          }}
        >
          <h3 style={{ color: "#ef4444" }}>error</h3>
          <p style={{ color: "#ef4444" }}>{error}</p>
        </div>
      )}

      {proof && verificationResult && (
        <>
          <div
            className="section"
            style={{
              borderColor: verificationResult.multiproofValid
                ? "#22c55e"
                : "#ef4444",
              backgroundColor: verificationResult.multiproofValid
                ? "rgba(34, 197, 94, 0.1)"
                : "rgba(239, 68, 68, 0.1)",
            }}
          >
            <h3
              style={{
                color: verificationResult.multiproofValid
                  ? "#22c55e"
                  : "#ef4444",
              }}
            >
              {verificationResult.multiproofValid
                ? "\u2713 batch proof valid"
                : "\u2717 batch proof invalid"}
            </h3>
            <p
              style={{ opacity: 0.8, marginTop: "0.5rem", fontSize: "0.9rem" }}
            >
              {proof.txHashes.length} transactions verified against shared
              checkpoint using Merkle multiproof.
            </p>
          </div>

          <div className="section">
            <h3>shared checkpoint</h3>
            <div className="staking-overview">
              <div className="stat-row">
                <span>checkpoint height:</span>
                <span className="value">
                  {proof.finality.checkpointHeight}
                </span>
              </div>
              <div className="stat-row">
                <span>checkpoint hash:</span>
                <span
                  className="value"
                  style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                >
                  {truncateHash(proof.finality.checkpointHash)}
                </span>
              </div>
              <div className="stat-row">
                <span>state root:</span>
                <span
                  className="value"
                  style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                >
                  {truncateHash(proof.finality.stateRoot)}
                </span>
              </div>
              <div className="stat-row">
                <span>timestamp:</span>
                <span className="value">
                  {new Date(proof.finality.checkpointTimestamp).toLocaleString()}
                </span>
              </div>
              <div className="stat-row">
                <span>BLS signature:</span>
                <span
                  className="value"
                  style={{
                    color: proof.finality.blsAggregatedSig
                      ? "#22c55e"
                      : "#888",
                  }}
                >
                  {proof.finality.blsAggregatedSig ? "present" : "none"}
                </span>
              </div>
              {proof.chainId && (
                <div className="stat-row">
                  <span>chain:</span>
                  <span className="value">{proof.chainId}</span>
                </div>
              )}
            </div>
          </div>

          {proof.freshness && (
            <div className="section">
              <h3>proof freshness</h3>
              <div className="staking-overview">
                <div className="stat-row">
                  <span>generated at checkpoint:</span>
                  <span className="value">
                    {proof.freshness.generatedAtCheckpoint}
                  </span>
                </div>
                <div className="stat-row">
                  <span>chain tip at generation:</span>
                  <span className="value">
                    {proof.freshness.chainTipAtGeneration}
                  </span>
                </div>
                <div className="stat-row">
                  <span>current chain tip:</span>
                  <span className="value">
                    {chainTip !== null ? chainTip : "unknown"}
                  </span>
                </div>
                {(() => {
                  const tip = chainTip ?? proof.freshness.chainTipAtGeneration;
                  const age = tip - proof.freshness.generatedAtCheckpoint;
                  return (
                    <div className="stat-row">
                      <span>age:</span>
                      <span
                        className="value"
                        style={{
                          color: freshnessColor(age),
                          fontWeight: "bold",
                        }}
                      >
                        {age} checkpoints ({freshnessLabel(age)})
                      </span>
                    </div>
                  );
                })()}
                <div className="stat-row">
                  <span>generated at:</span>
                  <span className="value">
                    {new Date(proof.freshness.generatedAtTimestamp).toLocaleString()}
                  </span>
                </div>
                {proof.freshness.maxAgeCheckpoints !== null && (
                  <div className="stat-row">
                    <span>max age allowed:</span>
                    <span className="value">
                      {proof.freshness.maxAgeCheckpoints} checkpoints
                    </span>
                  </div>
                )}
              </div>
            </div>
          )}

          <div className="section">
            <h3>multiproof details</h3>
            <div className="staking-overview">
              <div className="stat-row">
                <span>merkle root:</span>
                <span
                  className="value"
                  style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                >
                  {truncateHash(proof.multiproof.root)}
                </span>
              </div>
              <div className="stat-row">
                <span>total leaves in tree:</span>
                <span className="value">{proof.multiproof.numLeaves}</span>
              </div>
              <div className="stat-row">
                <span>proven leaves:</span>
                <span className="value">
                  {proof.multiproof.leafHashes.length}
                </span>
              </div>
              <div className="stat-row">
                <span>helper nodes:</span>
                <span className="value">
                  {proof.multiproof.helperHashes.length}
                </span>
              </div>
              <div className="stat-row">
                <span>verification:</span>
                <span
                  className="value"
                  style={{
                    color: verificationResult.multiproofValid
                      ? "#22c55e"
                      : "#ef4444",
                  }}
                >
                  {verificationResult.multiproofValid
                    ? "\u2713 valid"
                    : "\u2717 invalid"}
                </span>
              </div>
            </div>
          </div>

          <div className="section">
            <h3>transaction inclusion ({proof.txHashes.length})</h3>
            <div className="top-stakers">
              {proof.txHashes.map((hash, i) => {
                const isInProof = proof.multiproof.leafHashes.includes(hash);
                return (
                  <div
                    key={i}
                    style={{
                      padding: "6px 0",
                      borderBottom: "1px solid #333",
                      display: "flex",
                      justifyContent: "space-between",
                      alignItems: "center",
                    }}
                  >
                    <span
                      style={{
                        fontFamily: "monospace",
                        fontSize: "0.85rem",
                        color: "#88c0d0",
                      }}
                    >
                      {truncateHash(hash)}
                    </span>
                    <span
                      style={{
                        color: isInProof ? "#22c55e" : "#f59e0b",
                        fontSize: "0.85rem",
                      }}
                    >
                      {isInProof ? "\u2713 included" : "\u2713 referenced"}
                    </span>
                  </div>
                );
              })}
            </div>
          </div>
        </>
      )}
    </div>
  );
}
