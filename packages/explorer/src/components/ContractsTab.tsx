import { useState, useEffect, useRef } from "react";
import { useRinku } from "../context/WalletContext";
import { API_URL } from "../config";
import { useWebSocketContext } from "../context/WebSocketContext";
import { StateWitnessPanel } from "./StateWitnessPanel";

const NODE_URL = API_URL;

interface ContractSummary {
  contractId: string;
  creator: string;
  deployUrl: string;
  stateHash: string;
  height: number;
  createdAt: number;
  wasmBase64: string;
  state?: Record<string, unknown>;
}

interface ContractEvent {
  contractId: string;
  eventName: string;
  data: Record<string, unknown>;
  index: number;
}

interface StateChange {
  key: string;
  oldValue: unknown;
  newValue: unknown;
}

interface StateDiff {
  preHash: string;
  postHash: string;
  changes: StateChange[];
}

interface CallResult {
  success: boolean;
  gasUsed: number;
  error?: string;
  errorMessage?: string;
  logs: string[];
  events: ContractEvent[];
  stateDiff?: StateDiff;
  returnData?: string;
  newState?: Record<string, unknown>;
  newStateHash?: string;
  newHeight?: number;
}

export function ContractsTab() {
  const { wallet: keyPair, accountInfo, refreshAccount, submitTransaction } = useRinku();
  const walletReady = !!keyPair;
  const [contracts, setContracts] = useState<ContractSummary[]>([]);
  const [selectedContract, setSelectedContract] =
    useState<ContractSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const [wasmFile, setWasmFile] = useState<File | null>(null);
  const [initState, setInitState] = useState("{}");
  const fileInputRef = useRef<HTMLInputElement>(null);

  const [callEntrypoint, setCallEntrypoint] = useState("init");
  const [callInput, setCallInput] = useState("{}");
  const [callResult, setCallResult] = useState<CallResult | null>(null);

  const [expandedState, setExpandedState] = useState(false);

  const fetchContracts = async () => {
    try {
      const res = await fetch(`${NODE_URL}/contracts`);
      const data = await res.json();
      setContracts(data.contracts || []);
    } catch (e) {
      console.error("Failed to fetch contracts:", e);
    } finally {
      setLoading(false);
    }
  };

  const fetchContractDetails = async (contractId: string) => {
    try {
      const res = await fetch(`${NODE_URL}/contracts/${contractId}`);
      const data = await res.json();
      setSelectedContract(data);
      setCallResult(null);
      setError(null);
      setResult(null);
    } catch (e) {
      console.error("Failed to fetch contract:", e);
    }
  };

  const { status: wsStatus, lastBatch } = useWebSocketContext();
  const lastBatchIdRef = useRef(0);
  const contractRefreshRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    fetchContracts();
    return () => {
      if (contractRefreshRef.current) clearTimeout(contractRefreshRef.current);
    };
  }, []);

  useEffect(() => {
    if (!lastBatch || lastBatch.id === lastBatchIdRef.current) return;
    lastBatchIdRef.current = lastBatch.id;
    const relevant = lastBatch.items.some(e => e.type === 'CheckpointCreated' || e.type === 'FastPathExecuted');
    if (relevant && !contractRefreshRef.current) {
      contractRefreshRef.current = setTimeout(() => {
        contractRefreshRef.current = null;
        fetchContracts();
      }, 500);
    }
  }, [lastBatch]);

  useEffect(() => {
    if (wsStatus === 'connected') return;
    const interval = setInterval(fetchContracts, 5000);
    return () => clearInterval(interval);
  }, [wsStatus]);

  const fileToBase64 = (file: File): Promise<string> => {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        const arrayBuffer = reader.result as ArrayBuffer;
        const bytes = new Uint8Array(arrayBuffer);
        let binary = "";
        for (let i = 0; i < bytes.length; i++) {
          binary += String.fromCharCode(bytes[i]);
        }
        resolve(btoa(binary));
      };
      reader.onerror = reject;
      reader.readAsArrayBuffer(file);
    });
  };

  const handleDeploy = async () => {
    if (!walletReady || !keyPair) {
      setError("Connect a wallet first");
      return;
    }

    if (!wasmFile) {
      setError("Select a .wasm file to deploy");
      return;
    }

    let parsedInitState: Record<string, unknown>;
    try {
      parsedInitState = JSON.parse(initState);
    } catch {
      setError("Invalid JSON in init state");
      return;
    }

    setError(null);
    setResult(null);
    setSubmitting(true);

    try {
      const wasmBase64 = await fileToBase64(wasmFile);

      const contractData = JSON.stringify({
        action: "deploy",
        wasmBase64,
        initState: parsedInitState,
      });

      const txResult = await submitTransaction({
        to: "contract:deploy",
        amount: 0,
        kind: "contract",
        data: contractData,
      });

      setResult(`deploy tx submitted! hash: ${txResult.hash.slice(0, 16)}... (contract deploys on finalization)`);
      setWasmFile(null);
      setInitState("{}");
      if (fileInputRef.current) fileInputRef.current.value = "";
      setTimeout(() => {
        fetchContracts();
        refreshAccount();
      }, 3000);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSubmitting(false);
    }
  };

  const handleCall = async () => {
    if (!selectedContract) return;
    if (!walletReady || !keyPair) {
      setError("Connect a wallet first");
      return;
    }

    let parsedInput: Record<string, unknown>;
    try {
      parsedInput = JSON.parse(callInput);
    } catch {
      setError("Invalid JSON in call input");
      return;
    }

    setError(null);
    setResult(null);
    setCallResult(null);
    setSubmitting(true);

    try {
      const contractData = JSON.stringify({
        action: "call",
        contractId: selectedContract.contractId,
        entrypoint: callEntrypoint,
        input: parsedInput,
      });

      const txResult = await submitTransaction({
        to: selectedContract.contractId,
        amount: 0,
        kind: "contract",
        data: contractData,
      });

      setResult(`call tx submitted! hash: ${txResult.hash.slice(0, 16)}... (executes on finalization)`);
      setTimeout(() => {
        fetchContractDetails(selectedContract.contractId);
        fetchContracts();
        refreshAccount();
      }, 3000);
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSubmitting(false);
    }
  };

  const handleSimulateCall = async () => {
    if (!selectedContract) return;
    if (!walletReady || !keyPair) {
      setError("Connect a wallet first");
      return;
    }

    let parsedInput: Record<string, unknown>;
    try {
      parsedInput = JSON.parse(callInput);
    } catch {
      setError("Invalid JSON in call input");
      return;
    }

    setError(null);
    setResult(null);
    setCallResult(null);
    setSubmitting(true);

    try {
      const res = await fetch(
        `${NODE_URL}/contracts/${selectedContract.contractId}/call`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            caller: keyPair.fingerprint,
            entrypoint: callEntrypoint,
            input: parsedInput,
          }),
        },
      );

      const data: CallResult = await res.json();
      setCallResult(data);

      if (data.success) {
        setResult(`simulation ok — gas: ${data.gasUsed} (not committed)`);
      } else {
        setError(data.errorMessage || data.error || "Simulation failed");
      }
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSubmitting(false);
    }
  };

  const formatTime = (ts: number) => {
    return new Date(ts * 1000).toLocaleString();
  };

  const hasWasm =
    selectedContract?.wasmBase64 && selectedContract.wasmBase64.length > 0;

  if (loading) {
    return <div className="loading">loading contracts...</div>;
  }

  return (
    <div className="rewards-tab">
      <div className="section">
        <h3>smart contracts</h3>
        <div className="staking-overview">
          <div className="stat-row">
            <span>deployed contracts:</span>
            <span className="value">{contracts.length}</span>
          </div>
          <div className="stat-row">
            <span>runtime:</span>
            <span className="value">{"wasmi (wasm)"}</span>
          </div>
        </div>
      </div>

      <div className="section">
        <div className="wallet-section">
          <h4>wallet</h4>
          {!walletReady ? (
            <div className="wallet-connect-prompt">
              <p>
                connect a wallet using the wallet button in the header to deploy
                and interact with contracts.
              </p>
            </div>
          ) : (
            <div className="wallet-info">
              <div className="stat-row">
                <span>address:</span>
                <span className="value mono">
                  {keyPair?.fingerprint?.slice(0, 16)}...
                </span>
              </div>
              <div className="stat-row">
                <span>balance:</span>
                <span className="value">
                  {accountInfo?.balance?.toFixed(4) || "0.0000"} RKU
                </span>
              </div>
            </div>
          )}
        </div>
      </div>

      {(error || result) && (
        <div className="section" style={{ padding: "12px 20px" }}>
          {error && <div className="error">{error}</div>}
          {result && <div className="success">{result}</div>}
        </div>
      )}

      {walletReady && (
        <div className="section">
          <h3>deploy contract</h3>
          <div style={{ marginBottom: 8 }}>
            <label
              style={{
                display: "block",
                fontSize: 12,
                color: "#888",
                marginBottom: 4,
              }}
            >
              wasm binary (.wasm file)
            </label>
            <input
              ref={fileInputRef}
              type="file"
              accept=".wasm"
              onChange={(e) => setWasmFile(e.target.files?.[0] || null)}
              style={{
                background: "#000",
                border: "1px solid #333",
                color: "#d8dee9",
                padding: "6px 8px",
                width: "100%",
                fontFamily: "'Courier New', Courier, monospace",
                fontSize: 12,
              }}
            />
          </div>
          <div style={{ marginBottom: 8 }}>
            <label
              style={{
                display: "block",
                fontSize: 12,
                color: "#888",
                marginBottom: 4,
              }}
            >
              initial state (json)
            </label>
            <input
              type="text"
              placeholder='{"key": "value"}'
              value={initState}
              onChange={(e) => setInitState(e.target.value)}
              style={{
                background: "#000",
                border: "1px solid #333",
                color: "#a3be8c",
                padding: "8px 12px",
                width: "100%",
                fontFamily: "'Courier New', Courier, monospace",
                fontSize: 13,
                boxSizing: "border-box",
              }}
            />
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <button onClick={handleDeploy} disabled={submitting || !wasmFile}>
              {submitting ? "deploying..." : "deploy (signed tx)"}
            </button>
            {wasmFile && (
              <span style={{ fontSize: 11, color: "#888" }}>
                {wasmFile.name} ({(wasmFile.size / 1024).toFixed(1)} KB)
              </span>
            )}
          </div>
        </div>
      )}

      <div className="section">
        <h3>deployed contracts</h3>
        {contracts.length === 0 ? (
          <div className="empty">no contracts deployed yet</div>
        ) : (
          <div className="top-stakers">
            {contracts.map((c) => (
              <div
                key={c.contractId}
                className={`staker-row ${selectedContract?.contractId === c.contractId ? "selected" : ""}`}
                onClick={() => fetchContractDetails(c.contractId)}
                style={{
                  cursor: "pointer",
                  padding: "8px 0",
                  borderBottom: "1px solid #333",
                }}
              >
                <span className="mono" style={{ color: "#b48ead" }}>
                  {c.contractId}
                </span>
                <span className="value" style={{ fontSize: 11 }}>
                  h:{c.height} {c.wasmBase64?.length > 0 ? "wasm" : "mock"}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>

      {selectedContract && (
        <>
          <div className="section">
            <h3>contract: {selectedContract.contractId}</h3>
            <div className="staking-overview">
              <div className="stat-row">
                <span>creator:</span>
                <span className="value mono">
                  {selectedContract.creator.slice(0, 20)}...
                </span>
              </div>
              <div className="stat-row">
                <span>type:</span>
                <span className="value">
                  {hasWasm ? "wasm" : "mock"}
                  {hasWasm && (
                    <span style={{ color: "#666", marginLeft: 4 }}>
                      (
                      {Math.ceil(
                        (selectedContract.wasmBase64.length * 3) / 4 / 1024,
                      )}{" "}
                      KB)
                    </span>
                  )}
                </span>
              </div>
              <div className="stat-row">
                <span>height:</span>
                <span className="value">{selectedContract.height}</span>
              </div>
              <div className="stat-row">
                <span>created:</span>
                <span className="value">
                  {formatTime(selectedContract.createdAt)}
                </span>
              </div>
              <div className="stat-row">
                <span>state hash:</span>
                <span className="value mono">{selectedContract.stateHash}</span>
              </div>
              <div className="stat-row">
                <span>deploy url:</span>
                <span className="value mono" style={{ fontSize: 11 }}>
                  {selectedContract.deployUrl}
                </span>
              </div>
            </div>

            <div style={{ marginTop: 12 }}>
              <h4
                onClick={() => setExpandedState(!expandedState)}
                style={{ cursor: "pointer", userSelect: "none" }}
              >
                {expandedState ? "▼" : "▶"} contract state
              </h4>
              {expandedState && (
                <pre
                  style={{
                    background: "#000",
                    border: "1px solid #333",
                    padding: 12,
                    fontSize: 12,
                    color: "#a3be8c",
                    overflow: "auto",
                    maxHeight: 200,
                  }}
                >
                  {JSON.stringify(selectedContract.state || {}, null, 2)}
                </pre>
              )}
            </div>
          </div>

          {walletReady && (
            <div className="section">
              <h3>call contract</h3>
              <div style={{ marginBottom: 8 }}>
                <label
                  style={{
                    display: "block",
                    fontSize: 12,
                    color: "#888",
                    marginBottom: 4,
                  }}
                >
                  entrypoint
                </label>
                <input
                  type="text"
                  placeholder="init"
                  value={callEntrypoint}
                  onChange={(e) => setCallEntrypoint(e.target.value)}
                  style={{
                    background: "#000",
                    border: "1px solid #333",
                    color: "#a3be8c",
                    padding: "8px 12px",
                    width: "100%",
                    fontFamily: "'Courier New', Courier, monospace",
                    fontSize: 13,
                    boxSizing: "border-box",
                  }}
                />
              </div>
              <div style={{ marginBottom: 8 }}>
                <label
                  style={{
                    display: "block",
                    fontSize: 12,
                    color: "#888",
                    marginBottom: 4,
                  }}
                >
                  input (json)
                </label>
                <textarea
                  placeholder='{"key": "value"}'
                  value={callInput}
                  onChange={(e) => setCallInput(e.target.value)}
                  rows={3}
                  style={{
                    background: "#000",
                    border: "1px solid #333",
                    color: "#a3be8c",
                    padding: "8px 12px",
                    width: "100%",
                    fontFamily: "'Courier New', Courier, monospace",
                    fontSize: 13,
                    resize: "vertical",
                    boxSizing: "border-box",
                  }}
                />
              </div>
              <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
                <button onClick={handleCall} disabled={submitting}>
                  {submitting ? "submitting..." : "execute (signed tx)"}
                </button>
                <button onClick={handleSimulateCall} disabled={submitting} style={{ opacity: 0.8 }}>
                  {submitting ? "simulating..." : "simulate (dry run)"}
                </button>
                <span style={{ fontSize: 11, color: "#666" }}>
                  signed by {keyPair?.fingerprint?.slice(0, 8)}...
                </span>
              </div>
            </div>
          )}

          {callResult && (
            <div className="section">
              <h3>execution result</h3>
              <div className="staking-overview">
                <div className="stat-row">
                  <span>status:</span>
                  <span
                    className="value"
                    style={{
                      color: callResult.success ? "#a3be8c" : "#bf616a",
                    }}
                  >
                    {callResult.success ? "success" : "failed"}
                  </span>
                </div>
                <div className="stat-row">
                  <span>gas used:</span>
                  <span className="value">
                    {callResult.gasUsed.toLocaleString()}
                  </span>
                </div>
                {callResult.errorMessage && (
                  <div className="stat-row">
                    <span>error:</span>
                    <span className="value" style={{ color: "#bf616a" }}>
                      {callResult.errorMessage}
                    </span>
                  </div>
                )}
              </div>

              {callResult.logs.length > 0 && (
                <div style={{ marginTop: 12 }}>
                  <h4>logs</h4>
                  <pre
                    style={{
                      background: "#000",
                      border: "1px solid #333",
                      padding: 8,
                      fontSize: 11,
                      color: "#ebcb8b",
                      overflow: "auto",
                      maxHeight: 120,
                    }}
                  >
                    {callResult.logs.map((l, i) => `[${i}] ${l}`).join("\n")}
                  </pre>
                </div>
              )}

              {callResult.events.length > 0 && (
                <div style={{ marginTop: 12 }}>
                  <h4>events</h4>
                  {callResult.events.map((ev, i) => (
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
                      <span style={{ color: "#88c0d0" }}>{ev.eventName}</span>
                      <pre
                        style={{
                          color: "#a3be8c",
                          margin: "4px 0 0",
                          fontSize: 11,
                        }}
                      >
                        {JSON.stringify(ev.data, null, 2)}
                      </pre>
                    </div>
                  ))}
                </div>
              )}

              {callResult.stateDiff &&
                callResult.stateDiff.changes.length > 0 && (
                  <div style={{ marginTop: 12 }}>
                    <h4>state changes</h4>
                    <div
                      style={{
                        background: "#000",
                        border: "1px solid #333",
                        padding: 8,
                        fontSize: 12,
                      }}
                    >
                      {callResult.stateDiff.changes.map((ch, i) => (
                        <div key={i} style={{ marginBottom: 4 }}>
                          <span style={{ color: "#88c0d0" }}>{ch.key}</span>
                          <span style={{ color: "#666" }}>{" : "}</span>
                          <span style={{ color: "#bf616a" }}>
                            {ch.oldValue !== null
                              ? JSON.stringify(ch.oldValue)
                              : "null"}
                          </span>
                          <span style={{ color: "#666" }}>{" → "}</span>
                          <span style={{ color: "#a3be8c" }}>
                            {ch.newValue !== null
                              ? JSON.stringify(ch.newValue)
                              : "deleted"}
                          </span>
                        </div>
                      ))}
                    </div>
                    <div
                      style={{
                        fontSize: 11,
                        color: "#666",
                        marginTop: 4,
                      }}
                    >
                      {callResult.stateDiff.preHash} →{" "}
                      {callResult.stateDiff.postHash}
                    </div>
                  </div>
                )}

              {callResult.newState && (
                <div style={{ marginTop: 12 }}>
                  <h4>updated state</h4>
                  <pre
                    style={{
                      background: "#000",
                      border: "1px solid #333",
                      padding: 12,
                      fontSize: 12,
                      color: "#a3be8c",
                      overflow: "auto",
                      maxHeight: 200,
                    }}
                  >
                    {JSON.stringify(callResult.newState, null, 2)}
                  </pre>
                  {callResult.newStateHash && (
                    <div style={{ fontSize: 11, color: "#666", marginTop: 2 }}>
                      state hash: {callResult.newStateHash} | height:{" "}
                      {callResult.newHeight}
                    </div>
                  )}
                </div>
              )}
            </div>
          )}

          <StateWitnessPanel contractId={selectedContract.contractId} />
        </>
      )}
    </div>
  );
}
