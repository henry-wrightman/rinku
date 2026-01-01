import React, { useState, useEffect } from 'react';

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
    tips: string[];
    sig: string;
    ts: number;
    hash: string;
  };
  parents: string[];
  children: string[];
  weight: number;
  confirmed: boolean;
}

interface State {
  accounts: Account[];
  nodes: DAGNode[];
  tips: string[];
  merkleRoot: string;
}

const NODE_URL = '/api';

function App() {
  const [tab, setTab] = useState<'dag' | 'accounts' | 'wallet' | 'faucet'>('dag');
  const [state, setState] = useState<State | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [walletAddress, setWalletAddress] = useState('');
  const [faucetAddress, setFaucetAddress] = useState('');
  const [faucetMessage, setFaucetMessage] = useState<{ type: 'success' | 'error'; text: string } | null>(null);

  const fetchState = async () => {
    try {
      const [dagRes, accountsRes] = await Promise.all([
        fetch(`${NODE_URL}/dag`),
        fetch(`${NODE_URL}/accounts`)
      ]);

      const dagData = await dagRes.json() as { nodes: DAGNode[]; tips: string[]; merkleRoot: string };
      const accountsData = await accountsRes.json() as { accounts: Account[] };

      setState({
        accounts: accountsData.accounts,
        nodes: dagData.nodes,
        tips: dagData.tips,
        merkleRoot: dagData.merkleRoot
      });
      setError(null);
    } catch (err: any) {
      setError('Failed to connect to Rinku Node');
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
      setFaucetMessage({ type: 'error', text: 'Please enter an address' });
      return;
    }

    try {
      const res = await fetch('http://localhost:3002/api/request', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ address: faucetAddress })
      });

      const data = await res.json() as { amount?: number; txHash?: string; error?: string };

      if (res.ok && data.amount && data.txHash) {
        setFaucetMessage({ type: 'success', text: `Received ${data.amount} coins! TX: ${data.txHash.slice(0, 16)}...` });
        fetchState();
      } else {
        setFaucetMessage({ type: 'error', text: data.error || 'Unknown error' });
      }
    } catch (err) {
      setFaucetMessage({ type: 'error', text: 'Failed to connect to faucet' });
    }
  };

  const truncateHash = (hash: string, len = 8) => {
    if (hash.length <= len * 2) return hash;
    return `${hash.slice(0, len)}...${hash.slice(-len)}`;
  };

  if (loading) {
    return (
      <div className="container">
        <header>
          <h1>Rinku Explorer</h1>
          <p>URL-Native Distributed Ledger</p>
        </header>
        <div className="loading">
          <div className="spinner"></div>
          Loading...
        </div>
      </div>
    );
  }

  return (
    <div className="container">
      <header>
        <h1>Rinku Explorer</h1>
        <p>URL-Native Distributed Ledger</p>
      </header>

      <div className="stats-grid">
        <div className="stat-card">
          <div className="value">{state?.nodes.length || 0}</div>
          <div className="label">Transactions</div>
        </div>
        <div className="stat-card">
          <div className="value">{state?.accounts.length || 0}</div>
          <div className="label">Accounts</div>
        </div>
        <div className="stat-card">
          <div className="value">{state?.tips.length || 0}</div>
          <div className="label">Active Tips</div>
        </div>
        <div className="stat-card">
          <div className="value">{truncateHash(state?.merkleRoot || '---', 6)}</div>
          <div className="label">Merkle Root</div>
        </div>
      </div>

      <div className="tabs">
        <button className={`tab ${tab === 'dag' ? 'active' : ''}`} onClick={() => setTab('dag')}>
          DAG View
        </button>
        <button className={`tab ${tab === 'accounts' ? 'active' : ''}`} onClick={() => setTab('accounts')}>
          Accounts
        </button>
        <button className={`tab ${tab === 'faucet' ? 'active' : ''}`} onClick={() => setTab('faucet')}>
          Faucet
        </button>
      </div>

      {error && (
        <div className="alert alert-error">{error}</div>
      )}

      {tab === 'dag' && (
        <div className="section">
          <h2>Transaction DAG</h2>
          <div className="dag-container">
            {state?.nodes.length === 0 ? (
              <div className="empty-state">
                No transactions yet. Use the faucet to get started!
              </div>
            ) : (
              state?.nodes.map((node) => (
                <div key={node.tx.hash} className="dag-node">
                  <span className="tx-hash">{truncateHash(node.tx.hash)}</span>
                  <span className="tx-amount">{node.tx.amount} coins</span>
                  <span style={{ fontSize: '0.7rem', color: '#888' }}>
                    {node.tx.from === 'faucet' ? 'Faucet' : truncateHash(node.tx.from, 4)} → {truncateHash(node.tx.to, 4)}
                  </span>
                </div>
              ))
            )}
          </div>
        </div>
      )}

      {tab === 'accounts' && (
        <div className="section">
          <h2>Accounts</h2>
          {state?.accounts.length === 0 ? (
            <div className="empty-state">No accounts yet</div>
          ) : (
            <table>
              <thead>
                <tr>
                  <th>Address</th>
                  <th>Balance</th>
                  <th>Nonce</th>
                  <th>Created</th>
                </tr>
              </thead>
              <tbody>
                {state?.accounts.map((account) => (
                  <tr key={account.fingerprint}>
                    <td className="hash">{truncateHash(account.fingerprint, 10)}</td>
                    <td className="amount">{account.balance.toLocaleString()}</td>
                    <td>{account.nonce}</td>
                    <td>{new Date(account.firstTxTimestamp).toLocaleDateString()}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}

      {tab === 'faucet' && (
        <div className="section">
          <h2>Testnet Faucet</h2>
          <p style={{ color: '#888', marginBottom: '20px' }}>
            Get free testnet coins to try out Rinku. Each address can request coins once per minute.
          </p>

          {faucetMessage && (
            <div className={`alert alert-${faucetMessage.type}`}>
              {faucetMessage.text}
            </div>
          )}

          <div className="wallet-form">
            <div className="form-group">
              <label>Your Address (Fingerprint)</label>
              <input
                type="text"
                placeholder="Enter your wallet fingerprint..."
                value={faucetAddress}
                onChange={(e: React.ChangeEvent<HTMLInputElement>) => setFaucetAddress(e.target.value)}
              />
            </div>
            <button className="btn btn-primary" onClick={requestFaucet}>
              Request 100 Coins
            </button>
          </div>

          <p style={{ color: '#666', marginTop: '20px', fontSize: '0.9rem' }}>
            Tip: Generate a wallet fingerprint using the @rinku/wallet package or enter any 40-character hex string.
          </p>
        </div>
      )}
    </div>
  );
}

export default App;
