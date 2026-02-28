import { useState } from "react";
import { API_URL } from "../config";
import type { StateWitnessData, ProofFreshness } from "../crypto";
import { verifyStateWitness } from "../crypto";

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

interface StateWitnessResponse {
  success: boolean;
  witness: StateWitnessData | null;
  keyCount: number;
  error?: string;
}

export function StateWitnessPanel({
  contractId,
}: {
  contractId: string;
}) {
  const [keysInput, setKeysInput] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [witness, setWitness] = useState<StateWitnessData | null>(null);
  const [verResult, setVerResult] = useState<{
    valid: boolean;
    entryCount: number;
    entriesWithProof: number;
  } | null>(null);
  const [chainTip, setChainTip] = useState<number | null>(null);

  const truncateHash = (hash: string) => {
    if (hash.length <= 20) return hash;
    return `${hash.slice(0, 10)}...${hash.slice(-10)}`;
  };

  const fetchStateWitness = async () => {
    const keys = keysInput
      .split(/[\n,]/)
      .map((s) => s.trim())
      .filter((s) => s.length > 0);

    if (keys.length === 0) {
      setError("Enter at least one key");
      return;
    }

    setLoading(true);
    setError(null);
    setWitness(null);
    setVerResult(null);

    try {
      const res = await fetch(`${NODE_URL}/state/witness`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ contractId, keys }),
      });

      const raw = await res.json();

      if (!raw.success || !raw.witness) {
        setError(raw.error || "Failed to generate state witness");
        return;
      }

      const w = raw.witness;
      const normalized: StateWitnessData = {
        type: w.type,
        contractId: w.contractId ?? w.contract_id ?? null,
        entries: (w.entries ?? []).map((e: any) => ({
          key: e.key,
          value: e.value ?? null,
          proofKey: e.proofKey ?? e.proof_key ?? "",
          proofSiblings: e.proofSiblings ?? e.proof_siblings ?? [],
        })),
        stateRoot: w.stateRoot ?? w.state_root ?? "",
        checkpointHeight: w.checkpointHeight ?? w.checkpoint_height ?? 0,
        checkpointHash: w.checkpointHash ?? w.checkpoint_hash ?? "",
        blsAggregatedSig: w.blsAggregatedSig ?? w.bls_aggregated_sig ?? null,
        blsSignerBitmap: w.blsSignerBitmap ?? w.bls_signer_bitmap ?? null,
        chainId: w.chainId ?? w.chain_id ?? null,
        freshness: normalizeFreshness(w.freshness),
      };

      setWitness(normalized);

      try {
        const tipRes = await fetch(`${NODE_URL}/chain/tip`);
        const tipData = await tipRes.json();
        setChainTip(tipData.checkpointHeight ?? tipData.checkpoint_height ?? null);
      } catch {}


      const result = verifyStateWitness(normalized);
      setVerResult(result);
    } catch (e: any) {
      setError(e.message || "Failed to fetch state witness");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="section" style={{ marginTop: 12 }}>
      <h3>state witness</h3>
      <p style={{ opacity: 0.7, marginBottom: "0.75rem", fontSize: "0.85rem" }}>
        Generate a cryptographic state witness for specific contract keys.
        Enter storage keys separated by newlines or commas.
      </p>

      <textarea
        value={keysInput}
        onChange={(e) => setKeysInput(e.target.value)}
        placeholder="Enter storage keys (e.g., counter, owner, balances)"
        rows={3}
        style={{
          width: "100%",
          padding: "0.75rem",
          border: "1px solid #333",
          borderRadius: "4px",
          backgroundColor: "#000",
          color: "#a3be8c",
          fontFamily: "'Courier New', Courier, monospace",
          fontSize: "0.85rem",
          resize: "vertical",
          boxSizing: "border-box",
          marginBottom: "0.5rem",
        }}
      />

      <button onClick={fetchStateWitness} disabled={loading}>
        {loading ? "generating..." : "generate state witness"}
      </button>

      {error && (
        <div style={{ marginTop: 8 }}>
          <span style={{ color: "#ef4444", fontSize: "0.85rem" }}>{error}</span>
        </div>
      )}

      {witness && verResult && (
        <div style={{ marginTop: 12 }}>
          <div
            style={{
              padding: "8px 12px",
              marginBottom: 8,
              border: `1px solid ${verResult.valid ? "#22c55e" : "#ef4444"}`,
              backgroundColor: verResult.valid
                ? "rgba(34, 197, 94, 0.1)"
                : "rgba(239, 68, 68, 0.1)",
              borderRadius: 4,
            }}
          >
            <span
              style={{
                color: verResult.valid ? "#22c55e" : "#ef4444",
                fontWeight: "bold",
              }}
            >
              {verResult.valid
                ? "\u2713 state witness valid"
                : "\u2717 state witness invalid"}
            </span>
            <span
              style={{ marginLeft: 8, fontSize: "0.85rem", opacity: 0.8 }}
            >
              {verResult.entriesWithProof}/{verResult.entryCount} entries with
              proof
            </span>
          </div>

          <div className="staking-overview" style={{ marginBottom: 8 }}>
            <div className="stat-row">
              <span>checkpoint height:</span>
              <span className="value">{witness.checkpointHeight}</span>
            </div>
            <div className="stat-row">
              <span>checkpoint hash:</span>
              <span
                className="value"
                style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
              >
                {truncateHash(witness.checkpointHash)}
              </span>
            </div>
            <div className="stat-row">
              <span>state root:</span>
              <span
                className="value"
                style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
              >
                {truncateHash(witness.stateRoot)}
              </span>
            </div>
            <div className="stat-row">
              <span>BLS signature:</span>
              <span
                className="value"
                style={{
                  color: witness.blsAggregatedSig ? "#22c55e" : "#888",
                }}
              >
                {witness.blsAggregatedSig ? "present" : "none"}
              </span>
            </div>
          </div>

          {witness.freshness && (
            <div
              style={{
                padding: "8px 12px",
                marginBottom: 8,
                border: "1px solid #333",
                borderRadius: 4,
              }}
            >
              <h4 style={{ marginBottom: 6 }}>proof freshness</h4>
              <div className="staking-overview">
                <div className="stat-row">
                  <span>generated at checkpoint:</span>
                  <span className="value">
                    {witness.freshness.generatedAtCheckpoint}
                  </span>
                </div>
                <div className="stat-row">
                  <span>chain tip at generation:</span>
                  <span className="value">
                    {witness.freshness.chainTipAtGeneration}
                  </span>
                </div>
                <div className="stat-row">
                  <span>current chain tip:</span>
                  <span className="value">
                    {chainTip !== null ? chainTip : "unknown"}
                  </span>
                </div>
                {(() => {
                  const tip = chainTip ?? witness.freshness!.chainTipAtGeneration;
                  const age = tip - witness.freshness!.generatedAtCheckpoint;
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
                    {new Date(witness.freshness.generatedAtTimestamp).toLocaleString()}
                  </span>
                </div>
                {witness.freshness.maxAgeCheckpoints !== null && (
                  <div className="stat-row">
                    <span>max age allowed:</span>
                    <span className="value">
                      {witness.freshness.maxAgeCheckpoints} checkpoints
                    </span>
                  </div>
                )}
              </div>
            </div>
          )}

          <h4 style={{ marginBottom: 4 }}>
            key-value entries ({witness.entries.length})
          </h4>
          {witness.entries.map((entry, i) => (
            <div
              key={i}
              style={{
                background: "#000",
                border: "1px solid #333",
                padding: 8,
                marginBottom: 4,
                fontSize: 12,
              }}
            >
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                  marginBottom: 4,
                }}
              >
                <span style={{ color: "#88c0d0", fontWeight: "bold" }}>
                  {entry.key}
                </span>
                <span
                  style={{
                    color:
                      entry.proofSiblings.length > 0 ? "#22c55e" : "#f59e0b",
                    fontSize: 11,
                  }}
                >
                  {entry.proofSiblings.length > 0
                    ? `\u2713 proof (${entry.proofSiblings.length} siblings)`
                    : "no proof"}
                </span>
              </div>
              <div>
                <span style={{ color: "#888" }}>value: </span>
                <span style={{ color: "#a3be8c" }}>
                  {entry.value !== null && entry.value !== undefined
                    ? JSON.stringify(entry.value)
                    : "null"}
                </span>
              </div>
              <div style={{ marginTop: 2 }}>
                <span style={{ color: "#888" }}>proof key: </span>
                <span
                  style={{
                    color: "#b48ead",
                    fontFamily: "monospace",
                    fontSize: 11,
                  }}
                >
                  {truncateHash(entry.proofKey)}
                </span>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
