import React, { useState, useEffect } from "react";
import { Link } from "react-router-dom";

interface Account {
  fingerprint: string;
  balance: number;
  nonce: number;
  firstTxTimestamp: number;
}

interface DAGNode {
  tx: {
    from: string;
    to: string;
    amount: number;
    nonce: number;
    tipUrls: string[];
    sig: string;
    ts: number;
    hash: string;
  };
  parentUrls: string[];
  children: string[];
  weight: number;
  confirmed: boolean;
  url: string;
}

interface State {
  accounts: Account[];
  nodes: DAGNode[];
  tips: string[];
  tipUrls: string[];
  merkleRoot: string;
}

const NODE_URL = "/api";

function App() {
  const [tab, setTab] = useState<"dag" | "accounts" | "faucet">("dag");
  const [state, setState] = useState<State | null>(null);
  const [loading, setLoading] = useState(true);
  const [connected, setConnected] = useState(false);
  const [faucetAddress, setFaucetAddress] = useState("");
  const [faucetMessage, setFaucetMessage] = useState<{
    type: "success" | "error";
    text: string;
  } | null>(null);
  const [dagPage, setDagPage] = useState(0);
  const [accountsPage, setAccountsPage] = useState(0);
  const [darkMode, setDarkMode] = useState(true);
  const PAGE_SIZE = 20;

  useEffect(() => {
    document.body.classList.toggle('light', !darkMode);
  }, [darkMode]);

  const fetchState = async () => {
    try {
      const [dagRes, accountsRes] = await Promise.all([
        fetch(`${NODE_URL}/dag`),
        fetch(`${NODE_URL}/accounts`),
      ]);

      const dagData = (await dagRes.json()) as {
        nodes: DAGNode[];
        tips: string[];
        tipUrls: string[];
        merkleRoot: string;
      };
      const accountsData = (await accountsRes.json()) as {
        accounts: Account[];
      };

      setState({
        accounts: accountsData.accounts,
        nodes: dagData.nodes,
        tips: dagData.tips,
        tipUrls: dagData.tipUrls || [],
        merkleRoot: dagData.merkleRoot,
      });
      setConnected(true);
    } catch (e) {
      console.error('Failed to fetch state:', e);
      setConnected(false);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchState();
    const interval = setInterval(fetchState, 5000);
    return () => clearInterval(interval);
  }, []);

  const requestFaucet = async () => {
    if (!faucetAddress) {
      setFaucetMessage({ type: "error", text: "address required" });
      return;
    }

    try {
      const res = await fetch("/api/faucet/request", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ address: faucetAddress }),
      });

      const data = (await res.json()) as {
        amount?: number;
        txHash?: string;
        error?: string;
      };

      if (res.ok && data.amount && data.txHash) {
        setFaucetMessage({
          type: "success",
          text: `received ${data.amount} coins. tx: ${data.txHash.slice(0, 8)}...`,
        });
        fetchState();
      } else {
        setFaucetMessage({
          type: "error",
          text: data.error || "request failed",
        });
      }
    } catch {
      setFaucetMessage({ type: "error", text: "failed to connect to faucet" });
    }
  };

  const truncate = (s: string, len = 8) => {
    if (!s || s.length <= len) return s;
    return `${s.slice(0, len)}...`;
  };

  const timeAgo = (ts: number) => {
    const diff = Date.now() - ts;
    const mins = Math.floor(diff / 60000);
    if (mins < 1) return "just now";
    if (mins < 60) return `${mins}m ago`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}h ago`;
    return `${Math.floor(hours / 24)}d ago`;
  };

  if (loading) {
    return (
      <div className="container">
        <header>
          <h1>rinku explorer</h1>
          <p>url-native distributed ledger</p>
        </header>
        <div className="loading">loading...</div>
      </div>
    );
  }

  return (
    <div className="container">
      <header>
        <h1>rinku explorer</h1>
        <p>url-native distributed ledger</p>
        <div className="status-indicator">
          <span className={`status-dot ${connected ? 'connected' : 'disconnected'}`}></span>
          <span className={`status-text ${connected ? 'connected' : 'disconnected'}`}>{connected ? 'connected' : 'disconnected'}</span>
        </div>
      </header>

      <button className="theme-toggle" onClick={() => setDarkMode(!darkMode)}>
        {darkMode ? '☀' : '☾'}
      </button>

      <div className="stats">
        <span>
          <span className="value">{state?.nodes.length || 0}</span> transactions
        </span>
        <span>
          <span className="value">{state?.accounts.length || 0}</span> accounts
        </span>
        <span>
          <span className="value">{state?.tips.length || 0}</span> tips
        </span>
      </div>

      <div className="nav">
        <span
          className={tab === "dag" ? "active" : ""}
          onClick={() => setTab("dag")}
        >
          dag
        </span>
        <span
          className={tab === "accounts" ? "active" : ""}
          onClick={() => setTab("accounts")}
        >
          accounts
        </span>
        <span
          className={tab === "faucet" ? "active" : ""}
          onClick={() => setTab("faucet")}
        >
          faucet
        </span>
      </div>

      {tab === "dag" && (
        <div className="section">
          {state?.nodes.length === 0 ? (
            <div className="empty">no transactions yet</div>
          ) : (
            <>
              {state?.nodes
                .slice()
                .reverse()
                .slice(dagPage * PAGE_SIZE, (dagPage + 1) * PAGE_SIZE)
                .map((node) => (
                <div key={node.tx.hash} className="dag-node">
                  <div className="hash">{truncate(node.tx.hash, 12)}</div>
                  <div className="amount">
                    {node.tx.amount.toLocaleString()} coins
                  </div>
                  <div className="meta">
                    {node.tx.from === "genesis"
                      ? "genesis"
                      : truncate(node.tx.from, 6)}{" "}
                    → {truncate(node.tx.to, 6)} · {timeAgo(node.tx.ts)} · refs {(node.tx.tipUrls || []).length} parent(s)
                  </div>
                  <div className="actions">
                    <span className="link" onClick={() => {
                      const fullUrl = window.location.origin + node.url;
                      navigator.clipboard.writeText(fullUrl);
                      alert('Transaction URL copied!');
                    }}>copy url</span>
                    <Link to={node.url} className="link">view</Link>
                  </div>
                </div>
              ))}
              
              {state && state.nodes.length > PAGE_SIZE && (
                <div className="pagination">
                  <span 
                    className={`page-btn ${dagPage === 0 ? 'disabled' : ''}`}
                    onClick={() => dagPage > 0 && setDagPage(dagPage - 1)}
                  >← prev</span>
                  <span className="page-info">
                    page {dagPage + 1} of {Math.ceil(state.nodes.length / PAGE_SIZE)}
                  </span>
                  <span 
                    className={`page-btn ${(dagPage + 1) * PAGE_SIZE >= state.nodes.length ? 'disabled' : ''}`}
                    onClick={() => (dagPage + 1) * PAGE_SIZE < state.nodes.length && setDagPage(dagPage + 1)}
                  >next →</span>
                </div>
              )}
            </>
          )}

          {state && state.merkleRoot && (
            <div style={{ marginTop: 20, color: "#555", fontSize: 12 }}>
              merkle root: {truncate(state.merkleRoot, 16)}
            </div>
          )}
        </div>
      )}

      {tab === "accounts" && (
        <div className="section">
          {state?.accounts.length === 0 ? (
            <div className="empty">no accounts yet</div>
          ) : (
            <>
              <table>
                <thead>
                  <tr>
                    <th>address</th>
                    <th>balance</th>
                    <th>nonce</th>
                  </tr>
                </thead>
                <tbody>
                  {state?.accounts
                    .slice(accountsPage * PAGE_SIZE, (accountsPage + 1) * PAGE_SIZE)
                    .map((account) => (
                    <tr key={account.fingerprint}>
                      <td className="hash">
                        {truncate(account.fingerprint, 12)}
                      </td>
                      <td className="amount">
                        {account.balance.toLocaleString()}
                      </td>
                      <td>{account.nonce}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              
              {state && state.accounts.length > PAGE_SIZE && (
                <div className="pagination">
                  <span 
                    className={`page-btn ${accountsPage === 0 ? 'disabled' : ''}`}
                    onClick={() => accountsPage > 0 && setAccountsPage(accountsPage - 1)}
                  >← prev</span>
                  <span className="page-info">
                    page {accountsPage + 1} of {Math.ceil(state.accounts.length / PAGE_SIZE)}
                  </span>
                  <span 
                    className={`page-btn ${(accountsPage + 1) * PAGE_SIZE >= state.accounts.length ? 'disabled' : ''}`}
                    onClick={() => (accountsPage + 1) * PAGE_SIZE < state.accounts.length && setAccountsPage(accountsPage + 1)}
                  >next →</span>
                </div>
              )}
            </>
          )}
        </div>
      )}

      {tab === "faucet" && (
        <div className="section">
          <div className="hint">
            get testnet coins. rate limited to once per minute.
          </div>

          {faucetMessage && (
            <div className={`message ${faucetMessage.type}`}>
              {faucetMessage.text}
            </div>
          )}

          <input
            type="text"
            placeholder="paste your address (fingerprint)"
            value={faucetAddress}
            onChange={(e: React.ChangeEvent<HTMLInputElement>) =>
              setFaucetAddress(e.target.value)
            }
          />

          <button className="btn" onClick={requestFaucet}>
            request coins
          </button>

          <div style={{ marginTop: 24, color: "#444", fontSize: 12 }}>
            tip: use @rinku/wallet to generate an address
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
