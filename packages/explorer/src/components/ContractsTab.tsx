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
        setResult(`Deployed: ${data.contractId}`);
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
        setResult(`Success! Gas: ${data.gasUsed}, Logs: ${data.logs?.join(", ") || "none"}`);
        fetchContractDetails(selectedContract.contractId);
        fetchContracts();
      } else {
        setError(data.error || "Call failed");
      }
    } catch (e: any) {
      setError(e.message);
    }
  };

  if (loading) {
    return <div className="loading">loading contracts...</div>;
  }

  return (
    <div className="contracts-tab">
      <div className="deploy-section">
        <h3>Deploy Contract</h3>
        <div className="form-row">
          <input
            type="text"
            placeholder="Creator fingerprint"
            value={deployForm.creator}
            onChange={(e) => setDeployForm({ ...deployForm, creator: e.target.value })}
          />
        </div>
        <div className="form-row">
          <input
            type="text"
            placeholder='Initial state (JSON)'
            value={deployForm.initState}
            onChange={(e) => setDeployForm({ ...deployForm, initState: e.target.value })}
          />
        </div>
        <button onClick={handleDeploy}>Deploy Token Contract</button>
      </div>

      {error && <div className="error-msg">{error}</div>}
      {result && <div className="success-msg">{result}</div>}

      <div className="contracts-list">
        <h3>Deployed Contracts ({contracts.length})</h3>
        {contracts.length === 0 ? (
          <div className="empty">No contracts deployed yet</div>
        ) : (
          contracts.map((c) => (
            <div
              key={c.contractId}
              className={`contract-item ${selectedContract?.contractId === c.contractId ? "selected" : ""}`}
              onClick={() => fetchContractDetails(c.contractId)}
            >
              <div className="contract-id">{c.contractId}</div>
              <div className="contract-meta">
                <span>Height: {c.height}</span>
                <span>Creator: {c.creator.slice(0, 8)}...</span>
              </div>
            </div>
          ))
        )}
      </div>

      {selectedContract && (
        <div className="contract-details">
          <h3>Contract: {selectedContract.contractId}</h3>
          
          <div className="state-view">
            <h4>State (height {selectedContract.height})</h4>
            <pre>{JSON.stringify(selectedContract.state, null, 2)}</pre>
            <div className="state-hash">Hash: {selectedContract.stateHash}</div>
          </div>

          <div className="call-section">
            <h4>Call Contract</h4>
            <div className="form-row">
              <select
                value={callForm.entrypoint}
                onChange={(e) => setCallForm({ ...callForm, entrypoint: e.target.value })}
              >
                <option value="mint">mint</option>
                <option value="transfer">transfer</option>
                <option value="get_balance">get_balance</option>
              </select>
            </div>
            <div className="form-row">
              <input
                type="text"
                placeholder="Input (JSON)"
                value={callForm.input}
                onChange={(e) => setCallForm({ ...callForm, input: e.target.value })}
              />
            </div>
            <button onClick={handleCall}>Execute</button>
          </div>
        </div>
      )}
    </div>
  );
}
