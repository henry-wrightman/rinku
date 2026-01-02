import { useState, useEffect } from "react";

interface ContractSummary {
  contractId: string;
  creator: string;
  deployUrl: string;
  stateHash: string;
  height: number;
  createdAt: number;
}

interface ContractState {
  contractId: string;
  creator: string;
  wasmBase64: string;
  deployUrl: string;
  state: Record<string, unknown>;
  stateHash: string;
  height: number;
  createdAt: number;
}

const NODE_URL = "/api";

export function ContractsTab() {
  const [contracts, setContracts] = useState<ContractSummary[]>([]);
  const [selectedContract, setSelectedContract] = useState<ContractState | null>(null);
  const [loading, setLoading] = useState(true);
  const [deployForm, setDeployForm] = useState({ creator: "", initState: "{}" });
  const [callForm, setCallForm] = useState({ entrypoint: "mint", input: '{"to": "", "amount": 100}' });
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

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
    } catch (e) {
      console.error("Failed to fetch contract:", e);
    }
  };

  useEffect(() => {
    fetchContracts();
    const interval = setInterval(fetchContracts, 5000);
    return () => clearInterval(interval);
  }, []);

  const handleDeploy = async () => {
    setError(null);
    setResult(null);
    
    try {
      const res = await fetch(`${NODE_URL}/contracts/deploy`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          creator: deployForm.creator,
          wasmBase64: btoa("mock-wasm-bytecode"),
          initState: JSON.parse(deployForm.initState)
        })
      });
      
      const data = await res.json();
      
      if (data.success) {
        setResult(`deployed: ${data.contractId}`);
        setDeployForm({ creator: "", initState: "{}" });
        fetchContracts();
      } else {
        setError(data.error || "Deploy failed");
      }
    } catch (e: any) {
      setError(e.message);
    }
  };

  const handleCall = async () => {
    if (!selectedContract) return;
    
    setError(null);
    setResult(null);
    
    try {
      const res = await fetch(`${NODE_URL}/contracts/${selectedContract.contractId}/call`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          entrypoint: callForm.entrypoint,
          input: JSON.parse(callForm.input),
          caller: "explorer"
        })
      });
      
      const data = await res.json();
      
      if (data.success) {
        setResult(`success! gas: ${data.gasUsed}, logs: ${data.logs?.join(", ") || "none"}`);
        fetchContractDetails(selectedContract.contractId);
        fetchContracts();
      } else {
        setError(data.error || "Call failed");
      }
    } catch (e: any) {
      setError(e.message);
    }
  };

  const formatTime = (ts: number) => {
    return new Date(ts).toLocaleString();
  };

  if (loading) {
    return <div className="loading">loading contracts...</div>;
  }

  return (
    <div className="rewards-tab">
      <div className="section">
        <h3>network contracts</h3>
        <div className="staking-overview">
          <div className="stat-row">
            <span>deployed contracts:</span>
            <span className="value">{contracts.length}</span>
          </div>
          <div className="stat-row">
            <span>runtime:</span>
            <span className="value">mock wasm</span>
          </div>
          <div className="stat-row">
            <span>entrypoints:</span>
            <span className="value">mint, transfer, get_balance</span>
          </div>
        </div>
      </div>

      <div className="section">
        <h3>deploy contract</h3>
        <div className="form-row">
          <input
            type="text"
            placeholder="creator fingerprint"
            value={deployForm.creator}
            onChange={(e) => setDeployForm({ ...deployForm, creator: e.target.value })}
          />
        </div>
        <div className="form-row">
          <input
            type="text"
            placeholder='initial state (json)'
            value={deployForm.initState}
            onChange={(e) => setDeployForm({ ...deployForm, initState: e.target.value })}
          />
          <button onClick={handleDeploy} disabled={!deployForm.creator}>
            deploy
          </button>
        </div>
      </div>

      {(error || result) && (
        <div className="section" style={{ padding: "12px 20px" }}>
          {error && <div className="error">{error}</div>}
          {result && <div className="success">{result}</div>}
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
                style={{ cursor: "pointer", padding: "8px 0", borderBottom: "1px solid #333" }}
              >
                <span className="mono" style={{ color: "#b48ead" }}>{c.contractId}</span>
                <span className="value">height {c.height}</span>
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
                <span className="value mono">{selectedContract.creator.slice(0, 16)}...</span>
              </div>
              <div className="stat-row">
                <span>height:</span>
                <span className="value">{selectedContract.height}</span>
              </div>
              <div className="stat-row">
                <span>created:</span>
                <span className="value">{formatTime(selectedContract.createdAt)}</span>
              </div>
              <div className="stat-row">
                <span>state hash:</span>
                <span className="value mono">{selectedContract.stateHash.slice(0, 16)}...</span>
              </div>
            </div>

            <h4 style={{ marginTop: 16 }}>state</h4>
            <pre style={{
              background: "#000",
              border: "1px solid #333",
              padding: 12,
              fontSize: 12,
              color: "#a3be8c",
              overflow: "auto",
              maxHeight: 200
            }}>
              {JSON.stringify(selectedContract.state, null, 2)}
            </pre>
          </div>

          <div className="section">
            <h3>call contract</h3>
            <div className="form-row">
              <select
                value={callForm.entrypoint}
                onChange={(e) => setCallForm({ ...callForm, entrypoint: e.target.value })}
                style={{
                  flex: "none",
                  width: 140,
                  background: "#000",
                  border: "1px solid #333",
                  color: "#a3be8c",
                  padding: "8px 12px",
                  fontFamily: "'Courier New', Courier, monospace",
                  fontSize: 13
                }}
              >
                <option value="mint">mint</option>
                <option value="transfer">transfer</option>
                <option value="get_balance">get_balance</option>
              </select>
              <input
                type="text"
                placeholder="input (json)"
                value={callForm.input}
                onChange={(e) => setCallForm({ ...callForm, input: e.target.value })}
              />
              <button onClick={handleCall}>
                execute
              </button>
            </div>
          </div>
        </>
      )}
    </div>
  );
}
