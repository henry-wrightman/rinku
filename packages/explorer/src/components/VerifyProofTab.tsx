import { useState } from "react";
import { BatchProofPanel } from "./BatchProofPanel";

const getApiBaseUrl = () => {
  const envApiUrl = import.meta.env.VITE_API_URL;

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

interface FreshnessInfo {
  generatedAtCheckpoint: number;
  generatedAtTimestamp: number;
  chainTipAtGeneration: number;
  currentChainTip?: number;
  maxAgeCheckpoints?: number;
}

interface VOVerifyResult {
  proofType: string;
  valid: boolean;
  errors: string[];
  checkpointHeight: number;
  freshness?: FreshnessInfo;
  details: Record<string, unknown>;
}

interface TransactionVerifyResult {
  proofType: "transaction";
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

interface AccountStateVerifyResult {
  proofType: "account_state";
  valid: boolean;
  address: string;
  balance: number;
  nonce: number;
  staked: number;
  checkpointHeight: number;
  stateRoot: string;
  merkleIndex: number;
  merkleProof: string[];
}

interface DecodeError {
  valid: false;
  error: string;
  proofType?: string;
}

type VerifyResponse =
  | VOVerifyResult
  | TransactionVerifyResult
  | AccountStateVerifyResult
  | DecodeError;

function isDecodeError(resp: VerifyResponse): resp is DecodeError {
  return "error" in resp;
}

function isVOResult(resp: VerifyResponse): resp is VOVerifyResult {
  return "details" in resp && "freshness" in resp && !("error" in resp);
}

function isTransactionResult(
  resp: VerifyResponse,
): resp is TransactionVerifyResult {
  return (
    "proofType" in resp &&
    resp.proofType === "transaction" &&
    "txHash" in resp &&
    !("error" in resp)
  );
}

function isAccountStateResult(
  resp: VerifyResponse,
): resp is AccountStateVerifyResult {
  return (
    "proofType" in resp &&
    resp.proofType === "account_state" &&
    "address" in resp &&
    !("error" in resp)
  );
}

function getFreshnessAge(freshness: FreshnessInfo): number {
  const tip = freshness.currentChainTip ?? freshness.chainTipAtGeneration;
  return tip - freshness.generatedAtCheckpoint;
}

function getFreshnessColor(freshness: FreshnessInfo): string {
  const age = getFreshnessAge(freshness);
  if (age <= 2) return "#22c55e";
  if (age <= 10) return "#eab308";
  return "#ef4444";
}

function getFreshnessLabel(freshness: FreshnessInfo): string {
  const age = getFreshnessAge(freshness);
  if (age <= 2) return "fresh";
  if (age <= 10) return "recent";
  return "stale";
}

export function VerifyProofTab() {
  const [proofUrl, setProofUrl] = useState("");
  const [result, setResult] = useState<
    VOVerifyResult | TransactionVerifyResult | AccountStateVerifyResult | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const verifyProof = async () => {
    const trimmed = proofUrl.trim();
    if (!trimmed) {
      setError("Please enter a proof URL");
      return;
    }

    if (/^[a-f0-9]{64}$/i.test(trimmed)) {
      setError(
        "This looks like a transaction hash, not a proof URL. Proof URLs start with 'rinku://vo/'",
      );
      return;
    }
    if (trimmed.startsWith("/tx/") || trimmed.startsWith("rinku://tx")) {
      setError(
        "This is a transaction reference URL. To verify offline, you need a proof URL that starts with 'rinku://vo/'.",
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
    if (!hash || hash.length <= 20) return hash || "";
    return `${hash.slice(0, 10)}...${hash.slice(-10)}`;
  };

  const renderFreshness = (freshness?: FreshnessInfo) => {
    if (!freshness) return null;
    const color = getFreshnessColor(freshness);
    const label = getFreshnessLabel(freshness);
    const age = getFreshnessAge(freshness);
    const currentTip = freshness.currentChainTip ?? freshness.chainTipAtGeneration;

    return (
      <div className="section">
        <h3>proof freshness</h3>
        <div className="staking-overview">
          <div className="stat-row">
            <span>status:</span>
            <span className="value" style={{ color }}>
              {label} ({age} checkpoint{age !== 1 ? "s" : ""} old)
            </span>
          </div>
          <div className="stat-row">
            <span>generated at checkpoint:</span>
            <span className="value">{freshness.generatedAtCheckpoint}</span>
          </div>
          <div className="stat-row">
            <span>generated at:</span>
            <span className="value">
              {formatTimestamp(freshness.generatedAtTimestamp)}
            </span>
          </div>
          <div className="stat-row">
            <span>current chain tip:</span>
            <span className="value">{currentTip}</span>
          </div>
          {freshness.maxAgeCheckpoints !== undefined &&
            freshness.maxAgeCheckpoints !== null && (
              <div className="stat-row">
                <span>max age:</span>
                <span className="value">
                  {freshness.maxAgeCheckpoints} checkpoints
                </span>
              </div>
            )}
        </div>
      </div>
    );
  };

  const renderVOResult = (result: VOVerifyResult) => {
    const d = result.details;
    const isTx = result.proofType === "tx_finality" || result.proofType === "TxFinality";
    const isAccount = result.proofType === "account_proof" || result.proofType === "AccountProof";

    return (
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
            {result.valid ? "✓" : "✗"} {result.proofType} proof{" "}
            {result.valid ? "valid" : "invalid"}
          </h3>
          <p
            style={{ opacity: 0.7, marginTop: "0.25rem", fontSize: "0.85rem" }}
          >
            VerifiableObject ({result.proofType}) at checkpoint{" "}
            {result.checkpointHeight}
          </p>

          {result.errors && result.errors.length > 0 && (
            <div style={{ marginTop: "0.5rem" }}>
              {result.errors.map((err: string, i: number) => (
                <p key={i} style={{ color: "#ef4444", margin: "0.25rem 0" }}>
                  • {err}
                </p>
              ))}
            </div>
          )}
        </div>

        {renderFreshness(result.freshness)}

        {isTx && (
          <>
            <div className="section">
              <h3>transaction details</h3>
              <div className="staking-overview">
                <div className="stat-row">
                  <span>tx hash:</span>
                  <span
                    className="value"
                    style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                  >
                    {truncateHash(d.txHash as string)}
                  </span>
                </div>
                <div className="stat-row">
                  <span>from:</span>
                  <span
                    className="value"
                    style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                  >
                    {truncateHash(d.txFrom as string)}
                  </span>
                </div>
                <div className="stat-row">
                  <span>to:</span>
                  <span
                    className="value"
                    style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                  >
                    {truncateHash(d.txTo as string)}
                  </span>
                </div>
                <div className="stat-row">
                  <span>amount:</span>
                  <span className="value">{d.txAmount as number} RKU</span>
                </div>
                <div className="stat-row">
                  <span>nonce:</span>
                  <span className="value">{d.txNonce as number}</span>
                </div>
                {typeof d.txTimestamp === "number" && (
                  <div className="stat-row">
                    <span>timestamp:</span>
                    <span className="value">
                      {formatTimestamp(d.txTimestamp)}
                    </span>
                  </div>
                )}
              </div>
            </div>

            <div className="section">
              <h3>finality proof</h3>
              <div className="staking-overview">
                <div className="stat-row">
                  <span>checkpoint height:</span>
                  <span className="value">{result.checkpointHeight}</span>
                </div>
                {d.checkpointHash && (
                  <div className="stat-row">
                    <span>checkpoint id:</span>
                    <span
                      className="value"
                      style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                    >
                      {truncateHash(d.checkpointHash as string)}
                    </span>
                  </div>
                )}
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
                      color: (d.merkleVerified as boolean)
                        ? "#22c55e"
                        : "#ef4444",
                    }}
                  >
                    {(d.merkleVerified as boolean) ? "✓ valid" : "✗ invalid"}
                  </span>
                </div>
                <div className="stat-row">
                  <span>BLS signature:</span>
                  <span
                    className="value"
                    style={{
                      color: (d.blsVerified as boolean) ? "#22c55e" : "#ef4444",
                    }}
                  >
                    {(d.blsVerified as boolean) ? "✓ valid" : "✗ invalid"}
                  </span>
                </div>
                <div className="stat-row">
                  <span>validator set:</span>
                  <span
                    className="value"
                    style={{
                      color: (d.validatorSetVerified as boolean)
                        ? "#22c55e"
                        : "#ef4444",
                    }}
                  >
                    {(d.validatorSetVerified as boolean)
                      ? "✓ valid"
                      : "✗ invalid"}
                  </span>
                </div>
              </div>
            </div>

            <div className="section">
              <h3>consensus weight</h3>
              <div className="staking-overview">
                <div className="stat-row">
                  <span>signer count:</span>
                  <span className="value">
                    {d.signerCount as number} validators
                  </span>
                </div>
                {d.signerWeight !== undefined && (
                  <div className="stat-row">
                    <span>signer weight:</span>
                    <span className="value">
                      {(d.signerWeight as number).toFixed(2)}
                    </span>
                  </div>
                )}
                {d.totalWeight !== undefined && (
                  <div className="stat-row">
                    <span>total weight:</span>
                    <span className="value">
                      {(d.totalWeight as number).toFixed(2)}
                    </span>
                  </div>
                )}
                {d.signerWeight !== undefined &&
                  d.totalWeight !== undefined && (
                    <div className="stat-row">
                      <span>weight ratio:</span>
                      <span
                        className="value"
                        style={{
                          color:
                            (d.signerWeight as number) /
                              (d.totalWeight as number) >=
                            0.67
                              ? "#22c55e"
                              : "#ef4444",
                        }}
                      >
                        {(
                          ((d.signerWeight as number) /
                            (d.totalWeight as number)) *
                          100
                        ).toFixed(1)}
                        %
                        {(d.signerWeight as number) /
                          (d.totalWeight as number) >=
                        0.67
                          ? " (≥67%)"
                          : " (<67%)"}
                      </span>
                    </div>
                  )}
              </div>
            </div>
          </>
        )}

        {isAccount && (
          <>
            <div className="section">
              <h3>account state</h3>
              <div className="staking-overview">
                <div className="stat-row">
                  <span>address:</span>
                  <span
                    className="value"
                    style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                  >
                    {truncateHash(d.address as string)}
                  </span>
                </div>
                <div className="stat-row">
                  <span>balance:</span>
                  <span className="value" style={{ color: "#a3be8c" }}>
                    {(d.balance as number)?.toLocaleString()} RKU
                  </span>
                </div>
                <div className="stat-row">
                  <span>nonce:</span>
                  <span className="value">{d.nonce as number}</span>
                </div>
                <div className="stat-row">
                  <span>staked:</span>
                  <span className="value">
                    {(d.staked as number) > 0
                      ? `${(d.staked as number).toLocaleString()} RKU`
                      : "none"}
                  </span>
                </div>
                {d.isOnDemand === true && (
                  <div className="stat-row">
                    <span>type:</span>
                    <span className="value">on-demand proof</span>
                  </div>
                )}
              </div>
            </div>

            <div className="section">
              <h3>checkpoint verification</h3>
              <div className="staking-overview">
                <div className="stat-row">
                  <span>checkpoint height:</span>
                  <span className="value">{result.checkpointHeight}</span>
                </div>
                {d.checkpointHash && (
                  <div className="stat-row">
                    <span>checkpoint id:</span>
                    <span
                      className="value"
                      style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                    >
                      {truncateHash(d.checkpointHash as string)}
                    </span>
                  </div>
                )}
                <div className="stat-row">
                  <span>state root:</span>
                  <span
                    className="value"
                    style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
                  >
                    {truncateHash(d.stateRoot as string)}
                  </span>
                </div>
                <div className="stat-row">
                  <span>merkle index:</span>
                  <span className="value">{d.merkleIndex as number}</span>
                </div>
                <div className="stat-row">
                  <span>proof depth:</span>
                  <span className="value">{d.proofDepth as number} levels</span>
                </div>
              </div>
            </div>
          </>
        )}

        {!isTx && !isAccount && d.note && (
          <div className="section">
            <h3>details</h3>
            <p style={{ opacity: 0.7 }}>{d.note as string}</p>
          </div>
        )}
      </>
    );
  };

  const renderTransactionResult = (result: TransactionVerifyResult) => (
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
          {result.valid
            ? "✓ transaction proof valid"
            : "✗ transaction proof invalid"}
        </h3>
        {/* <p style={{ opacity: 0.7, marginTop: "0.25rem", fontSize: "0.85rem" }}>
          Legacy proof (rinku://sp/)
        </p> */}

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
            <span className="value">{formatTimestamp(result.txTimestamp)}</span>
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
              style={{ color: result.merkleVerified ? "#22c55e" : "#ef4444" }}
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
              {((result.signerWeight / result.totalWeight) * 100).toFixed(1)}%
              {result.signerWeight / result.totalWeight >= 0.67
                ? " (≥67%)"
                : " (<67%)"}
            </span>
          </div>
        </div>
      </div>
    </>
  );

  const renderAccountStateResult = (result: AccountStateVerifyResult) => (
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
          {result.valid
            ? "✓ account state proof valid"
            : "✗ account state proof invalid"}
        </h3>
        <p style={{ opacity: 0.8, marginTop: "0.5rem", fontSize: "0.9rem" }}>
          Proof (rinku://asp/) at checkpoint {result.checkpointHeight}.
        </p>
      </div>

      <div className="section">
        <h3>account state</h3>
        <div className="staking-overview">
          <div className="stat-row">
            <span>address:</span>
            <span
              className="value"
              style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
            >
              {truncateHash(result.address)}
            </span>
          </div>
          <div className="stat-row">
            <span>balance:</span>
            <span className="value" style={{ color: "#a3be8c" }}>
              {result.balance.toLocaleString()} RKU
            </span>
          </div>
          <div className="stat-row">
            <span>nonce:</span>
            <span className="value">{result.nonce}</span>
          </div>
          <div className="stat-row">
            <span>staked:</span>
            <span className="value">
              {result.staked > 0
                ? `${result.staked.toLocaleString()} RKU`
                : "none"}
            </span>
          </div>
        </div>
      </div>

      <div className="section">
        <h3>checkpoint verification</h3>
        <div className="staking-overview">
          <div className="stat-row">
            <span>checkpoint height:</span>
            <span className="value">{result.checkpointHeight}</span>
          </div>
          <div className="stat-row">
            <span>state root:</span>
            <span
              className="value"
              style={{ fontFamily: "monospace", fontSize: "0.85rem" }}
            >
              {truncateHash(result.stateRoot)}
            </span>
          </div>
          <div className="stat-row">
            <span>merkle index:</span>
            <span className="value">{result.merkleIndex}</span>
          </div>
          <div className="stat-row">
            <span>proof depth:</span>
            <span className="value">{result.merkleProof.length} levels</span>
          </div>
        </div>
      </div>
    </>
  );

  return (
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
      <div className="section">
        <h3>verify proof URL</h3>
        <p style={{ opacity: 0.7, marginBottom: "1rem", fontSize: "0.9rem" }}>
          Paste a proof URL to verify:
          <br />
          <code>rinku://vo/...</code> (unified VerifiableObject).
        </p>

        <div style={{ display: "flex", gap: "0.5rem", marginBottom: "1rem" }}>
          <textarea
            value={proofUrl}
            onChange={(e) => setProofUrl(e.target.value)}
            placeholder="rinku://vo/..."
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

      {result && isVOResult(result) && renderVOResult(result)}
      {result && isTransactionResult(result) && renderTransactionResult(result)}
      {result &&
        isAccountStateResult(result) &&
        renderAccountStateResult(result)}

      <BatchProofPanel />
    </div>
  );
}
