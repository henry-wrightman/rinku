import { useState } from "react";

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
const API_URL = getApiBaseUrl();

interface VerifyResult {
  valid: boolean;
  errors: string[];
  txHash: string;
  txFrom: string;
  txTo: string;
  txAmount: number;
  txNonce: number;
  txTimestamp: number;
  checkpointHeight: number;
  checkpointId: string;
  merkleVerified: boolean;
  blsVerified: boolean;
  validatorSetVerified: boolean;
  signerWeight: number;
  totalWeight: number;
  signerCount: number;
}

interface DecodeError {
  valid: false;
  error: string;
}

type VerifyResponse = VerifyResult | DecodeError;

function isDecodeError(resp: VerifyResponse): resp is DecodeError {
  return "error" in resp;
}

export function VerifyProofTab() {
  const [proofUrl, setProofUrl] = useState("");
  const [result, setResult] = useState<VerifyResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const verifyProof = async () => {
    const trimmed = proofUrl.trim();
    if (!trimmed) {
      setError("Please enter a proof URL");
      return;
    }

    // Check if user pasted just a hash or reference URL instead of a proof
    if (/^[a-f0-9]{64}$/i.test(trimmed)) {
      setError(
        "This looks like a transaction hash, not a proof URL. Self-contained proofs start with 'rinku://sp/' and contain compressed cryptographic data.",
      );
      return;
    }
    if (trimmed.startsWith("/tx/") || trimmed.startsWith("rinku://tx")) {
      setError(
        "This is a transaction reference URL. To verify offline, you need a self-contained proof URL that starts with 'rinku://sp/' and contains all cryptographic data.",
      );
      return;
    }

    setLoading(true);
    setError(null);
    setResult(null);

    try {
      const res = await fetch(`${API_URL}/verify-proof`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ proofUrl: proofUrl.trim() }),
      });

      const data: VerifyResponse = await res.json();

      if (isDecodeError(data)) {
        setError(data.error);
      } else {
        setResult(data);
      }
    } catch (e) {
      setError("Failed to verify proof. Make sure the node is running.");
    } finally {
      setLoading(false);
    }
  };

  const formatTimestamp = (ts: number) => {
    return new Date(ts).toLocaleString();
  };

  const truncateHash = (hash: string) => {
    if (hash.length <= 20) return hash;
    return `${hash.slice(0, 10)}...${hash.slice(-10)}`;
  };

  return (
    <div
      className="tx-proof"
      style={{
        marginTop: 24,
        padding: 20,
        background: "rgba(136, 192, 208, 0.1)",
        borderRadius: 8,
        border: "1px solid rgba(136, 192, 208, 0.3)",
        marginBottom: 20,
      }}
    >
      <div className="section">
        <h3>verify proof URL</h3>
        <p style={{ opacity: 0.7, marginBottom: "1rem", fontSize: "0.9rem" }}>
          Paste a self-contained proof URL (<code>rinku://sp/...</code>) to
          verify a transaction.
        </p>

        <div style={{ display: "flex", gap: "0.5rem", marginBottom: "1rem" }}>
          <textarea
            value={proofUrl}
            onChange={(e) => setProofUrl(e.target.value)}
            placeholder="rinku://sp/..."
            rows={4}
            style={{
              flex: 1,
              padding: "0.75rem",
              border: "1px solid var(--border)",
              borderRadius: "4px",
              backgroundColor: "var(--bg-secondary)",
              color: "var(--text-primary)",
              fontFamily: "monospace",
              fontSize: "0.85rem",
              resize: "vertical",
            }}
          />
        </div>

        <button
          onClick={verifyProof}
          disabled={loading}
          className="btn-proof btn-proof-verify"
        >
          {loading ? "verifying..." : "verify proof"}
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
          <h3 style={{ color: "#ef4444" }}>verification failed</h3>
          <p style={{ color: "#ef4444" }}>{error}</p>
        </div>
      )}

      {result && (
        <>
          <div
            className="section"
            style={{
              borderColor: result.valid ? "#22c55e" : "#ef4444",
              backgroundColor: result.valid
                ? "rgba(34, 197, 94, 0.1)"
                : "rgba(239, 68, 68, 0.1)",
            }}
          >
            <h3 style={{ color: result.valid ? "#22c55e" : "#ef4444" }}>
              {result.valid ? "✓ proof valid" : "✗ proof invalid"}
            </h3>

            {result.errors.length > 0 && (
              <div style={{ marginTop: "0.5rem" }}>
                {result.errors.map((err, i) => (
                  <p key={i} style={{ color: "#ef4444", margin: "0.25rem 0" }}>
                    • {err}
                  </p>
                ))}
              </div>
            )}
          </div>

          <div className="section">
            <h3>transaction details</h3>
            <div className="staking-overview">
              <div className="stat-row">
                <span>tx hash:</span>
                <span
                  className="value"
                  style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                >
                  {truncateHash(result.txHash)}
                </span>
              </div>
              <div className="stat-row">
                <span>from:</span>
                <span
                  className="value"
                  style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                >
                  {truncateHash(result.txFrom)}
                </span>
              </div>
              <div className="stat-row">
                <span>to:</span>
                <span
                  className="value"
                  style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                >
                  {truncateHash(result.txTo)}
                </span>
              </div>
              <div className="stat-row">
                <span>amount:</span>
                <span className="value">{result.txAmount} RKU</span>
              </div>
              <div className="stat-row">
                <span>nonce:</span>
                <span className="value">{result.txNonce}</span>
              </div>
              <div className="stat-row">
                <span>timestamp:</span>
                <span className="value">
                  {formatTimestamp(result.txTimestamp)}
                </span>
              </div>
            </div>
          </div>

          <div className="section">
            <h3>finality proof</h3>
            <div className="staking-overview">
              <div className="stat-row">
                <span>checkpoint height:</span>
                <span className="value">{result.checkpointHeight}</span>
              </div>
              <div className="stat-row">
                <span>checkpoint id:</span>
                <span
                  className="value"
                  style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                >
                  {truncateHash(result.checkpointId)}
                </span>
              </div>
            </div>
          </div>

          <div className="section">
            <h3>cryptographic verification</h3>
            <div className="staking-overview">
              <div className="stat-row">
                <span>merkle proof:</span>
                <span
                  className="value"
                  style={{
                    color: result.merkleVerified ? "#22c55e" : "#ef4444",
                  }}
                >
                  {result.merkleVerified ? "✓ valid" : "✗ invalid"}
                </span>
              </div>
              <div className="stat-row">
                <span>BLS signature:</span>
                <span
                  className="value"
                  style={{ color: result.blsVerified ? "#22c55e" : "#ef4444" }}
                >
                  {result.blsVerified ? "✓ valid" : "✗ invalid"}
                </span>
              </div>
              <div className="stat-row">
                <span>validator set:</span>
                <span
                  className="value"
                  style={{
                    color: result.validatorSetVerified ? "#22c55e" : "#ef4444",
                  }}
                >
                  {result.validatorSetVerified ? "✓ valid" : "✗ invalid"}
                </span>
              </div>
            </div>
          </div>

          <div className="section">
            <h3>consensus weight</h3>
            <div className="staking-overview">
              <div className="stat-row">
                <span>signer count:</span>
                <span className="value">{result.signerCount} validators</span>
              </div>
              <div className="stat-row">
                <span>signer weight:</span>
                <span className="value">{result.signerWeight.toFixed(2)}</span>
              </div>
              <div className="stat-row">
                <span>total weight:</span>
                <span className="value">{result.totalWeight.toFixed(2)}</span>
              </div>
              <div className="stat-row">
                <span>weight ratio:</span>
                <span
                  className="value"
                  style={{
                    color:
                      result.signerWeight / result.totalWeight >= 0.67
                        ? "#22c55e"
                        : "#ef4444",
                  }}
                >
                  {((result.signerWeight / result.totalWeight) * 100).toFixed(
                    1,
                  )}
                  %
                  {result.signerWeight / result.totalWeight >= 0.67
                    ? " (≥67%)"
                    : " (<67%)"}
                </span>
              </div>
            </div>
          </div>
        </>
      )}
    </div>
  );
}
