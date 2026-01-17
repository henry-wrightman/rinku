import { useState, useEffect } from "react";
import {
  generateKeyPair,
  deserializeKeyPair,
  serializeKeyPair,
  createSignedTransaction,
  type SerializedKeyPair,
} from "../crypto";

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

interface AccountInfo {
  fingerprint: string;
  balance: number;
  nonce: number;
  staked: number;
}

const NODE_URL = "/api";
const WALLET_STORAGE_KEY = "rinku_contracts_wallet";

export function ContractsTab() {
  const [contracts, setContracts] = useState<ContractSummary[]>([]);
  const [selectedContract, setSelectedContract] =
    useState<ContractState | null>(null);
  const [loading, setLoading] = useState(true);
  const [deployForm, setDeployForm] = useState({ initState: "{}" });
  const [callForm, setCallForm] = useState({
    entrypoint: "mint",
    input: '{"to": "", "amount": 100}',
  });
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const [keyPair, setKeyPair] = useState<SerializedKeyPair | null>(null);
  const [walletReady, setWalletReady] = useState(false);
  const [importKey, setImportKey] = useState("");
  const [accountInfo, setAccountInfo] = useState<AccountInfo | null>(null);
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    const stored = localStorage.getItem(WALLET_STORAGE_KEY);
    if (stored) {
      try {
        const kp = deserializeKeyPair(stored);
        setKeyPair(kp);
        setWalletReady(true);
      } catch (e) {
        console.error("Failed to load stored wallet:", e);
      }
    }
  }, []);

  useEffect(() => {
    if (walletReady && keyPair) {
      fetchAccountInfo();
      const interval = setInterval(fetchAccountInfo, 5000);
      return () => clearInterval(interval);
    }
  }, [walletReady, keyPair]);

  const fetchAccountInfo = async () => {
    if (!keyPair) return;
    try {
      const res = await fetch(`${NODE_URL}/account/${keyPair.fingerprint}`);
      if (res.ok) {
        const data = await res.json();
        setAccountInfo(data);
      } else {
        setAccountInfo({
          fingerprint: keyPair.fingerprint,
          balance: 0,
          nonce: 0,
          staked: 0,
        });
      }
    } catch {
      setAccountInfo({
        fingerprint: keyPair.fingerprint,
        balance: 0,
        nonce: 0,
        staked: 0,
      });
    }
  };

  const handleImportWallet = async () => {
    setError(null);
    setResult(null);
    try {
      const kp = deserializeKeyPair(importKey);
      setKeyPair(kp);
      localStorage.setItem(WALLET_STORAGE_KEY, serializeKeyPair(kp));
      setWalletReady(true);
      setImportKey("");
      setResult(`Wallet imported! Address: ${kp.fingerprint.slice(0, 16)}...`);
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : "Unknown error";
      setError(`Import failed: ${message}`);
    }
  };

  const handleGenerateWallet = async () => {
    setError(null);
    setResult(null);
    try {
      const kp = await generateKeyPair();
      setKeyPair(kp);
      localStorage.setItem(WALLET_STORAGE_KEY, serializeKeyPair(kp));
      setWalletReady(true);
      setResult(
        `Wallet created! Address: ${kp.fingerprint.slice(0, 16)}... SAVE YOUR KEY!`,
      );
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : "Unknown error";
      setError(`Generation failed: ${message}`);
    }
  };

  const handleClearWallet = () => {
    setKeyPair(null);
    setAccountInfo(null);
    setWalletReady(false);
    localStorage.removeItem(WALLET_STORAGE_KEY);
    setSelectedContract(null);
    setResult("Wallet cleared");
  };

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

  const getTips = async (): Promise<string[]> => {
    try {
      const res = await fetch(`${NODE_URL}/tipUrls`);
      const data = await res.json();
      return (data.tipUrls || []).slice(0, 5);
    } catch {
      return [];
    }
  };

  const getGasPrice = async (): Promise<number> => {
    try {
      const res = await fetch(`${NODE_URL}/gas/price`);
      const data = await res.json();
      return data.current || 0.01;
    } catch {
      return 0.01;
    }
  };

  const handleDeploy = async () => {
    if (!walletReady || !keyPair) {
      setError("Set up a wallet first");
      return;
    }

    setError(null);
    setResult(null);
    setSubmitting(true);

    try {
      const tips = await getTips();
      const fee = await getGasPrice();

      const accountRes = await fetch(
        `${NODE_URL}/account/${keyPair.fingerprint}`,
      );
      const account = await accountRes.json();
      const nonce = account.nonce || 0;

      const signedTx = await createSignedTransaction(keyPair, {
        to: "contract:deploy",
        amount: 0,
        nonce,
        parents: tips,
        kind: "contract",
        gasPrice: fee,
      });

      const res = await fetch(`${NODE_URL}/tx`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(signedTx),
      });

      const data = await res.json();

      if (res.ok && data.hash) {
        setResult(
          `Contract deploy submitted (tx: ${data.hash.slice(0, 12)}...)`,
        );
        setDeployForm({ initState: "{}" });
        setTimeout(() => {
          fetchContracts();
          fetchAccountInfo();
        }, 1000);
      } else {
        setError(data.error || "Deploy failed");
      }
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSubmitting(false);
    }
  };

  const handleCall = async () => {
    if (!selectedContract) return;
    if (!walletReady || !keyPair) {
      setError("Set up a wallet first");
      return;
    }

    setError(null);
    setResult(null);
    setSubmitting(true);

    try {
      const tips = await getTips();
      const fee = await getGasPrice();

      const accountRes = await fetch(
        `${NODE_URL}/account/${keyPair.fingerprint}`,
      );
      const account = await accountRes.json();
      const nonce = account.nonce || 0;

      const signedTx = await createSignedTransaction(keyPair, {
        to: `contract:${selectedContract.contractId}`,
        amount: 0,
        nonce,
        parents: tips,
        kind: "contract",
        gasPrice: fee,
      });

      const res = await fetch(`${NODE_URL}/tx`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(signedTx),
      });

      const data = await res.json();

      if (res.ok && data.hash) {
        setResult(`Contract call submitted (tx: ${data.hash.slice(0, 12)}...)`);
        setTimeout(() => {
          fetchContractDetails(selectedContract.contractId);
          fetchContracts();
          fetchAccountInfo();
        }, 1000);
      } else {
        setError(data.error || "Call failed");
      }
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSubmitting(false);
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
        <div className="wallet-section">
          <h4>wallet</h4>
          {!walletReady ? (
            <div className="wallet-setup">
              <button onClick={handleGenerateWallet} className="primary">
                generate new wallet
              </button>
              <div style={{ margin: "12px 0", color: "#666", fontSize: 12 }}>
                or import an existing wallet from CLI
              </div>
              <div className="form-row">
                <input
                  type="text"
                  placeholder='paste wallet JSON {"publicKey":"...","privateKey":"...","fingerprint":"..."}'
                  value={importKey}
                  onChange={(e) => setImportKey(e.target.value)}
                  style={{ flex: 1 }}
                />
                <button onClick={handleImportWallet} disabled={!importKey}>
                  import
                </button>
              </div>
            </div>
          ) : (
            <div className="wallet-info">
              <div className="stat-row">
                <span>address:</span>
                <span className="value mono">{keyPair?.fingerprint}</span>
              </div>
              <div className="stat-row">
                <span>balance:</span>
                <span className="value">
                  {accountInfo?.balance?.toLocaleString() || "0.00"} RKU
                </span>
              </div>
              <div className="stat-row">
                <span>nonce:</span>
                <span className="value">{accountInfo?.nonce || 0}</span>
              </div>
              <div style={{ marginTop: 8 }}>
                <button
                  onClick={handleClearWallet}
                  style={{ background: "#3b4252", fontSize: 11 }}
                >
                  disconnect wallet
                </button>
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
          <div className="form-row">
            <input
              type="text"
              placeholder="initial state (json)"
              value={deployForm.initState}
              onChange={(e) =>
                setDeployForm({ ...deployForm, initState: e.target.value })
              }
            />
            <button onClick={handleDeploy} disabled={submitting}>
              {submitting ? "deploying..." : "deploy"}
            </button>
          </div>
          <div style={{ fontSize: 11, color: "#666", marginTop: 4 }}>
            Contract will be deployed from your connected wallet
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
                <span className="value mono">
                  {selectedContract.creator.slice(0, 16)}...
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
                <span className="value mono">
                  {selectedContract.stateHash.slice(0, 16)}...
                </span>
              </div>
            </div>

            <h4 style={{ marginTop: 16 }}>state</h4>
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
              {JSON.stringify(selectedContract.state, null, 2)}
            </pre>
          </div>

          {walletReady && (
            <div className="section">
              <h3>call contract</h3>
              <div className="form-row">
                <select
                  value={callForm.entrypoint}
                  onChange={(e) =>
                    setCallForm({ ...callForm, entrypoint: e.target.value })
                  }
                  style={{
                    flex: "none",
                    width: 140,
                    background: "#000",
                    border: "1px solid #333",
                    color: "#a3be8c",
                    padding: "8px 12px",
                    fontFamily: "'Courier New', Courier, monospace",
                    fontSize: 13,
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
                  onChange={(e) =>
                    setCallForm({ ...callForm, input: e.target.value })
                  }
                />
                <button onClick={handleCall} disabled={submitting}>
                  {submitting ? "executing..." : "execute"}
                </button>
              </div>
              <div style={{ fontSize: 11, color: "#666", marginTop: 4 }}>
                Contract call will be signed by your connected wallet
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}
