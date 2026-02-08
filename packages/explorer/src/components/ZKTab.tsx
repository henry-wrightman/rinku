import { useState, useEffect, useRef, useCallback } from "react";

const ZK_ARTIFACTS_URL = import.meta.env.VITE_ZK_ARTIFACTS_URL || "";
const VKEY_FILE = "verification_key.json";

function getNodeUrl(): string {
  const envApiUrl = import.meta.env.VITE_API_URL;
  if (
    envApiUrl &&
    typeof envApiUrl === "string" &&
    !envApiUrl.includes("localhost") &&
    !envApiUrl.includes("127.0.0.1")
  ) {
    return envApiUrl.replace(/\/+$/, "");
  }
  return "/api";
}

interface ZKStatus {
  enabled: boolean;
  version: number;
  chainId: string;
  artifactsAvailable: boolean;
  features: {
    witnessGeneration: boolean;
    proofVerification: boolean;
    proofGeneration: boolean;
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

interface ProofResult {
  success: boolean;
  zkUrl?: string;
  proofTime?: number;
  checkpointHeight?: number;
  nullifier?: string;
  error?: string;
}

interface CdnStatus {
  available: boolean;
  wasmUrl?: string;
  zkeyUrl?: string;
  vkeyLoaded: boolean;
  checking: boolean;
}

export function ZKTab() {
  const NODE_URL = getNodeUrl();
  const [status, setStatus] = useState<ZKStatus | null>(null);
  const [txHash, setTxHash] = useState("");
  const [privateKeySeed, setPrivateKeySeed] = useState("");
  const [witness, setWitness] = useState<MerkleWitness | null>(null);
  const [zkUrl, setZkUrl] = useState("");
  const [generatedProof, setGeneratedProof] = useState<ProofResult | null>(null);
  const [verifyResult, setVerifyResult] = useState<{
    valid?: boolean;
    error?: string;
    message?: string;
    verifiedLocally?: boolean;
    verifyTimeMs?: number;
  } | null>(null);
  const [loading, setLoading] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [cdnStatus, setCdnStatus] = useState<CdnStatus>({
    available: false,
    vkeyLoaded: false,
    checking: true,
  });
  const vkeyRef = useRef<object | null>(null);
  const snarkjsRef = useRef<typeof import("snarkjs") | null>(null);

  const loadSnarkjs = useCallback(async () => {
    if (snarkjsRef.current) return snarkjsRef.current;
    const sjs = await import("snarkjs");
    snarkjsRef.current = sjs;
    return sjs;
  }, []);

  useEffect(() => {
    fetchStatus();
    if (ZK_ARTIFACTS_URL) {
      checkCdnArtifacts();
    } else {
      setCdnStatus({ available: false, vkeyLoaded: false, checking: false });
    }
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

  const checkCdnArtifacts = async () => {
    setCdnStatus((prev) => ({ ...prev, checking: true }));
    try {
      const wasmUrl = `${ZK_ARTIFACTS_URL}/rinku_private_proof.wasm`;
      const zkeyUrl = `${ZK_ARTIFACTS_URL}/rinku_private_proof.zkey`;
      const vkeyUrl = `${ZK_ARTIFACTS_URL}/${VKEY_FILE}`;

      const [wasmRes, zkeyRes, vkeyRes] = await Promise.all([
        fetch(wasmUrl, { method: "GET", headers: { Range: "bytes=0-0" } }).catch(() => null),
        fetch(zkeyUrl, { method: "GET", headers: { Range: "bytes=0-0" } }).catch(() => null),
        fetch(vkeyUrl).catch(() => null),
      ]);

      let vkeyLoaded = false;
      if (vkeyRes && vkeyRes.ok) {
        try {
          const vkey = await vkeyRes.json();
          vkeyRef.current = vkey;
          vkeyLoaded = true;
          await loadSnarkjs();
        } catch {
          console.error("Failed to parse verification key");
        }
      }

      const wasmAvailable = wasmRes !== null && (wasmRes.ok || wasmRes.status === 206);
      const zkeyAvailable = zkeyRes !== null && (zkeyRes.ok || zkeyRes.status === 206);

      setCdnStatus({
        available: wasmAvailable && zkeyAvailable && vkeyLoaded,
        wasmUrl: wasmAvailable ? wasmUrl : undefined,
        zkeyUrl: zkeyAvailable ? zkeyUrl : undefined,
        vkeyLoaded,
        checking: false,
      });
    } catch (e) {
      console.error("CDN artifact check failed:", e);
      setCdnStatus({ available: false, vkeyLoaded: false, checking: false });
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

  const generateProof = async () => {
    if (!txHash) {
      setError("Transaction hash required");
      return;
    }

    setGenerating(true);
    setError(null);
    setGeneratedProof(null);

    try {
      const payload: { txHash: string; privateKeySeed?: string } = { txHash };
      if (privateKeySeed.trim()) {
        payload.privateKeySeed = privateKeySeed.trim();
      }
      const res = await fetch(`${NODE_URL}/zk/prove`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      const data = await res.json();

      if (!res.ok) {
        setError(data.error || "Failed to generate proof");
        return;
      }

      setGeneratedProof(data);
      if (data.zkUrl) {
        setZkUrl(data.zkUrl);
      }
    } catch (e) {
      setError("Failed to connect to node");
    } finally {
      setGenerating(false);
    }
  };

  const verifyProof = async () => {
    if (!zkUrl) {
      setVerifyResult({ error: "ZK URL required" });
      return;
    }

    setLoading(true);
    setVerifyResult(null);

    if (cdnStatus.vkeyLoaded && vkeyRef.current) {
      try {
        const snarkjs = await loadSnarkjs();

        let proof, publicSignals;

        if (zkUrl.startsWith("rinku://zk/")) {
          const qIdx = zkUrl.indexOf("?");
          if (qIdx === -1) throw new Error("Invalid ZK URL: no query params");
          const params = new URLSearchParams(zkUrl.substring(qIdx + 1));
          const proofStr = params.get("proof");
          const signalsStr = params.get("signals");

          if (proofStr && signalsStr) {
            proof = JSON.parse(atob(proofStr));
            publicSignals = JSON.parse(atob(signalsStr));
          } else {
            throw new Error("Invalid ZK URL: missing proof or signals");
          }
        } else {
          throw new Error("URL must start with rinku://zk/");
        }

        const startTime = performance.now();
        const isValid = await snarkjs.groth16.verify(
          vkeyRef.current,
          publicSignals,
          proof
        );
        const verifyTime = Math.round(performance.now() - startTime);

        setVerifyResult({
          valid: isValid,
          message: isValid
            ? `Proof cryptographically verified in ${verifyTime}ms`
            : "Proof verification failed — invalid or tampered",
          verifiedLocally: true,
          verifyTimeMs: verifyTime,
        });
      } catch (e) {
        try {
          const res = await fetch(`${NODE_URL}/zk/verify`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ zkUrl }),
          });
          const data = await res.json();
          setVerifyResult(data);
        } catch {
          setVerifyResult({
            error: e instanceof Error ? e.message : "Verification failed",
          });
        }
      }
    } else {
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
      }
    }

    setLoading(false);
  };

  return (
    <div className="rewards-tab">
      <div className="section">
        <h3>zk privacy layer</h3>
        <p className="section-description">
          Generate Groth16 zk-SNARK proofs that cryptographically hide sender,
          recipient, and amount while proving transaction validity, all without
          needing to share the original. Proofs verify in ~10ms offline via
          rinku://zk/ URLs.
        </p>

        <div className="staking-overview">
          {cdnStatus.checking ? (
            <div className="stat-row">
              <span>cdn artifacts:</span>
              <span className="value">checking...</span>
            </div>
          ) : ZK_ARTIFACTS_URL ? (
            <>
              <div className="stat-row">
                <span>cdn artifacts:</span>
                <span className="value" style={{ color: cdnStatus.available ? "#4ade80" : "#f87171" }}>
                  {cdnStatus.available ? "available" : "unavailable"}
                </span>
              </div>
              {cdnStatus.available && (
                <>
                  <div className="stat-row">
                    <span>circuit WASM:</span>
                    <span className="value" style={{ color: "#4ade80" }}>hosted on CDN</span>
                  </div>
                  <div className="stat-row">
                    <span>proving key:</span>
                    <span className="value" style={{ color: "#4ade80" }}>hosted on CDN</span>
                  </div>
                </>
              )}
              <div className="stat-row">
                <span>local verification:</span>
                <span className="value" style={{ color: cdnStatus.vkeyLoaded ? "#4ade80" : "#f87171" }}>
                  {cdnStatus.vkeyLoaded ? "ready (vkey loaded)" : "unavailable"}
                </span>
              </div>
            </>
          ) : null}

          {status && (
            <>
              <div className="stat-row">
                <span>status:</span>
                <span className="value">
                  {status.enabled ? "enabled" : "disabled"}
                </span>
              </div>
              <div className="stat-row">
                <span>chain id:</span>
                <span className="value">{status.chainId}</span>
              </div>
              <div className="stat-row">
                <span>protocol:</span>
                <span className="value">
                  {status.circuitInfo.protocol} ({status.circuitInfo.curve})
                </span>
              </div>
              <div className="stat-row">
                <span>merkle depth:</span>
                <span className="value">
                  {status.circuitInfo.merkleDepth} levels
                </span>
              </div>
              <div className="stat-row">
                <span>witness generation:</span>
                <span className="value">
                  {status.features.witnessGeneration ? "ready" : "unavailable"}
                </span>
              </div>
              <div className="stat-row">
                <span>proof generation:</span>
                <span className="value">
                  {status.features.proofGeneration
                    ? "ready"
                    : "pending artifacts"}
                </span>
              </div>
              <div className="stat-row">
                <span>proof verification:</span>
                <span className="value">
                  {cdnStatus.vkeyLoaded
                    ? "client-side (instant)"
                    : status.features.proofVerification
                      ? "server-side"
                      : "pending artifacts"}
                </span>
              </div>
            </>
          )}
        </div>
      </div>

      <div className="section">
        <h3>generate merkle witness</h3>
        <p className="section-description">
          Get the Merkle proof for a finalized transaction. This is the first
          step to create a ZK proof.
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
                <span className="value mono">
                  {witness.txHash.slice(0, 16)}...
                </span>
              </div>
              <div className="stat-row">
                <span>checkpoint:</span>
                <span className="value">#{witness.checkpointHeight}</span>
              </div>
              <div className="stat-row">
                <span>merkle root:</span>
                <span className="value mono">
                  {witness.checkpointRoot.slice(0, 16)}...
                </span>
              </div>
              <div className="stat-row">
                <span>proof length:</span>
                <span className="value">
                  {witness.merklePathElements.length} nodes
                </span>
              </div>
            </div>

            <details className="proof-details">
              <summary>raw proof data</summary>
              <pre className="json-output">
                {JSON.stringify(witness, null, 2)}
              </pre>
            </details>
          </div>
        )}
      </div>

      <div className="section">
        <h3>generate zk proof</h3>
        <p className="section-description">
          Generate a privacy-preserving ZK proof for a finalized transaction.
          This creates a <code>rinku://zk/...</code> URL that proves validity
          without revealing details.
        </p>

        <div
          className="form-group"
          style={{ flexDirection: "column", alignItems: "stretch" }}
        >
          <input
            type="text"
            placeholder="transaction hash (e.g., a1b2c3d4...)"
            value={txHash}
            onChange={(e) => setTxHash(e.target.value)}
            className="input-field"
            style={{ width: "100%", marginBottom: "0.5rem" }}
          />
          <input
            type="password"
            placeholder="private key seed (optional - your secret passphrase)"
            value={privateKeySeed}
            onChange={(e) => setPrivateKeySeed(e.target.value)}
            className="input-field"
            style={{ width: "100%", marginBottom: "0.5rem" }}
          />
          <p
            style={{
              fontSize: "0.75rem",
              opacity: 0.7,
              margin: "0 0 0.75rem",
              lineHeight: 1.4,
            }}
          >
            Your seed derives your ZK keypair. Use any memorable phrase. Leave
            empty for demo mode.
            <br />
            <span style={{ color: "#ffaa00" }}>Note:</span> In this demo, the
            seed is sent to the server for proof generation.
          </p>
          <button
            onClick={generateProof}
            disabled={
              generating || !txHash || !status?.features.proofGeneration
            }
            className="btn btn-primary"
            style={{ alignSelf: "flex-start" }}
          >
            {generating ? "generating proof..." : "generate zk proof"}
          </button>
        </div>

        {status && !status.features.proofGeneration && (
          <div className="warning-message">
            {status.artifactsAvailable
              ? "ZK prover initializing... Please wait a few seconds."
              : "ZK artifacts not yet compiled. Proof generation unavailable."}
          </div>
        )}

        {generatedProof && generatedProof.success && (
          <div className="proof-result success">
            <h4>proof generated</h4>
            <div className="code-block">
              <div className="stat-row">
                <span>proof time:</span>
                <span className="value">{generatedProof.proofTime}ms</span>
              </div>
              <div className="stat-row">
                <span>checkpoint:</span>
                <span className="value">
                  #{generatedProof.checkpointHeight}
                </span>
              </div>
              <div className="stat-row">
                <span>nullifier:</span>
                <span className="value mono">
                  {generatedProof.nullifier?.slice(0, 16)}...
                </span>
              </div>
            </div>
            <div className="zk-url-display">
              <label>zk url (click to copy):</label>
              <div
                className="zk-url mono"
                onClick={() => {
                  if (generatedProof.zkUrl) {
                    navigator.clipboard.writeText(generatedProof.zkUrl);
                  }
                }}
                title="Click to copy"
              >
                {generatedProof.zkUrl?.slice(0, 60)}...
              </div>
            </div>
          </div>
        )}
      </div>

      <div className="section">
        <h3>verify zk proof</h3>
        <p className="section-description">
          Verify a <code>rinku://zk/...</code> URL.{" "}
          {cdnStatus.vkeyLoaded
            ? "Verification runs entirely in your browser using the CDN-loaded verification key — no network requests needed."
            : "Verification works offline and proves transaction validity without revealing details."}
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
            {loading
              ? "verifying..."
              : cdnStatus.vkeyLoaded
                ? "verify (local)"
                : "verify"}
          </button>
        </div>

        {verifyResult && (
          <div
            className={`verify-result ${verifyResult.valid ? "success" : verifyResult.error ? "error" : "warning"}`}
          >
            {verifyResult.valid !== undefined && (
              <div className="result-status">
                {verifyResult.valid ? "Valid proof" : "Invalid proof"}
                {verifyResult.verifiedLocally && (
                  <span
                    style={{
                      fontSize: "0.75rem",
                      marginLeft: "0.5rem",
                      color: "#4ade80",
                    }}
                  >
                    (verified locally in {verifyResult.verifyTimeMs}ms)
                  </span>
                )}
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
              <p>
                Fetch the Merkle proof from any node for a finalized
                transaction.
              </p>
            </div>
          </div>
          <div className="step">
            <div className="step-number">2</div>
            <div className="step-content">
              <strong>Generate Proof</strong>
              <p>
                Run the ZK prover with your private key seed. The proof takes
                ~500ms on the server.
              </p>
            </div>
          </div>
          <div className="step">
            <div className="step-number">3</div>
            <div className="step-content">
              <strong>Share URL</strong>
              <p>
                The <code>rinku://zk/...</code> URL is your proof. Anyone can
                verify it offline.
              </p>
            </div>
          </div>
          <div className="step">
            <div className="step-number">4</div>
            <div className="step-content">
              <strong>Verify Anywhere</strong>
              <p>
                {cdnStatus.vkeyLoaded
                  ? "Verification runs in-browser using the CDN verification key — no server contact needed. Takes <10ms."
                  : "Verification takes <10ms and proves the transaction without revealing details."}
              </p>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
