import { useState, useEffect } from "react";

interface ZKStatus {
  enabled: boolean;
  version: number;
  chainId: string;
  artifactsAvailable: boolean;
  features: {
    witnessGeneration: boolean;
    proofVerification: boolean;
    nullifierRegistry: boolean;
  };
  circuitInfo: {
    merkleDepth: number;
    protocol: string;
    curve: string;
  };
}

interface MerkleWitness {
  txHash: string;
  merklePathElements: string[];
  merklePathIndices: number[];
  checkpointHeight: number;
  checkpointRoot: string;
  checkpointId: string;
  chainId: string;
}

const NODE_URL = "/api";

export function ZKTab() {
  const [status, setStatus] = useState<ZKStatus | null>(null);
  const [txHash, setTxHash] = useState("");
  const [witness, setWitness] = useState<MerkleWitness | null>(null);
  const [zkUrl, setZkUrl] = useState("");
  const [verifyResult, setVerifyResult] = useState<{
    valid?: boolean;
    error?: string;
    message?: string;
  } | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    fetchStatus();
  }, []);

  const fetchStatus = async () => {
    try {
      const res = await fetch(`${NODE_URL}/zk/status`);
      const data = await res.json();
      setStatus(data);
    } catch (e) {
      console.error("Failed to fetch ZK status:", e);
    }
  };

  const fetchWitness = async () => {
    if (!txHash) {
      setError("Transaction hash required");
      return;
    }

    setLoading(true);
    setError(null);
    setWitness(null);

    try {
      const res = await fetch(`${NODE_URL}/zk/witness/${txHash}`);
      const data = await res.json();

      if (!res.ok) {
        setError(data.error || "Failed to fetch witness");
        return;
      }

      setWitness(data);
    } catch (e) {
      setError("Failed to connect to node");
    } finally {
      setLoading(false);
    }
  };

  const verifyProof = async () => {
    if (!zkUrl) {
      setVerifyResult({ error: "ZK URL required" });
      return;
    }

    setLoading(true);
    setVerifyResult(null);

    try {
      const res = await fetch(`${NODE_URL}/zk/verify`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ zkUrl }),
      });
      const data = await res.json();
      setVerifyResult(data);
    } catch (e) {
      setVerifyResult({ error: "Failed to connect to node" });
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="rewards-tab">
      <div className="section">
        <h3>zk privacy layer</h3>
        <p className="section-description">
          Generate privacy-preserving proofs for transactions. ZK proofs hide sender, 
          recipient, and amount while proving transaction validity.
        </p>

        {status && (
          <div className="staking-overview">
            <div className="stat-row">
              <span>status:</span>
              <span className={`value ${status.enabled ? "success" : "warning"}`}>
                {status.enabled ? "enabled" : "disabled"}
              </span>
            </div>
            <div className="stat-row">
              <span>chain id:</span>
              <span className="value">{status.chainId}</span>
            </div>
            <div className="stat-row">
              <span>protocol:</span>
              <span className="value">{status.circuitInfo.protocol} ({status.circuitInfo.curve})</span>
            </div>
            <div className="stat-row">
              <span>merkle depth:</span>
              <span className="value">{status.circuitInfo.merkleDepth} levels</span>
            </div>
            <div className="stat-row">
              <span>witness generation:</span>
              <span className={`value ${status.features.witnessGeneration ? "success" : "error"}`}>
                {status.features.witnessGeneration ? "ready" : "unavailable"}
              </span>
            </div>
            <div className="stat-row">
              <span>proof verification:</span>
              <span className={`value ${status.features.proofVerification ? "success" : "warning"}`}>
                {status.features.proofVerification ? "ready" : "pending artifacts"}
              </span>
            </div>
          </div>
        )}
      </div>

      <div className="section">
        <h3>generate merkle witness</h3>
        <p className="section-description">
          Get the Merkle proof for a finalized transaction. This is the first step 
          to create a ZK proof.
        </p>

        <div className="form-group">
          <input
            type="text"
            placeholder="transaction hash (e.g., a1b2c3d4...)"
            value={txHash}
            onChange={(e) => setTxHash(e.target.value)}
            className="input-field"
          />
          <button
            onClick={fetchWitness}
            disabled={loading || !txHash}
            className="btn"
          >
            {loading ? "loading..." : "get witness"}
          </button>
        </div>

        {error && <div className="error-message">{error}</div>}

        {witness && (
          <div className="witness-result">
            <h4>merkle witness</h4>
            <div className="code-block">
              <div className="stat-row">
                <span>tx hash:</span>
                <span className="value mono">{witness.txHash.slice(0, 16)}...</span>
              </div>
              <div className="stat-row">
                <span>checkpoint:</span>
                <span className="value">#{witness.checkpointHeight}</span>
              </div>
              <div className="stat-row">
                <span>merkle root:</span>
                <span className="value mono">{witness.checkpointRoot.slice(0, 16)}...</span>
              </div>
              <div className="stat-row">
                <span>proof length:</span>
                <span className="value">{witness.merklePathElements.length} nodes</span>
              </div>
            </div>

            <details className="proof-details">
              <summary>raw proof data</summary>
              <pre className="json-output">
                {JSON.stringify(witness, null, 2)}
              </pre>
            </details>

            <div className="next-steps">
              <p>
                <strong>Next step:</strong> Use this witness with a local prover to generate 
                a ZK proof. The prover runs on your device for privacy.
              </p>
              <code className="cli-example">
                rinku zk prove --witness &lt;above-json&gt; --key &lt;your-private-key&gt;
              </code>
            </div>
          </div>
        )}
      </div>

      <div className="section">
        <h3>verify zk proof</h3>
        <p className="section-description">
          Verify a <code>rinku://zk/...</code> URL. Verification works offline and 
          proves transaction validity without revealing details.
        </p>

        <div className="form-group">
          <input
            type="text"
            placeholder="rinku://zk/..."
            value={zkUrl}
            onChange={(e) => setZkUrl(e.target.value)}
            className="input-field"
          />
          <button
            onClick={verifyProof}
            disabled={loading || !zkUrl}
            className="btn"
          >
            {loading ? "verifying..." : "verify"}
          </button>
        </div>

        {verifyResult && (
          <div className={`verify-result ${verifyResult.valid ? "success" : verifyResult.error ? "error" : "warning"}`}>
            {verifyResult.valid !== undefined && (
              <div className="result-status">
                {verifyResult.valid ? "Valid proof" : "Invalid proof"}
              </div>
            )}
            {verifyResult.error && (
              <div className="result-error">{verifyResult.error}</div>
            )}
            {verifyResult.message && (
              <div className="result-message">{verifyResult.message}</div>
            )}
          </div>
        )}
      </div>

      <div className="section">
        <h3>how it works</h3>
        <div className="how-it-works">
          <div className="step">
            <div className="step-number">1</div>
            <div className="step-content">
              <strong>Get Witness</strong>
              <p>Fetch the Merkle proof from any node for a finalized transaction.</p>
            </div>
          </div>
          <div className="step">
            <div className="step-number">2</div>
            <div className="step-content">
              <strong>Generate Proof</strong>
              <p>Run the ZK prover locally with your private key. The proof takes ~500ms.</p>
            </div>
          </div>
          <div className="step">
            <div className="step-number">3</div>
            <div className="step-content">
              <strong>Share URL</strong>
              <p>The <code>rinku://zk/...</code> URL is your proof. Anyone can verify it offline.</p>
            </div>
          </div>
          <div className="step">
            <div className="step-number">4</div>
            <div className="step-content">
              <strong>Verify Anywhere</strong>
              <p>Verification takes &lt;10ms and proves the transaction without revealing details.</p>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
