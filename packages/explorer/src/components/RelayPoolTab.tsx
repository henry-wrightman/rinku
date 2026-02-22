import { useState, useEffect, useCallback } from "react";
import { API_URL } from "../config";

const NODE_URL = API_URL;

interface Relayer {
  address: string;
  nodeUrl: string;
  feeRate: number;
  stake: number;
  relaysCompleted: number;
  lastSeen: number;
  isHealthy: boolean;
}

interface RelayPoolData {
  relayers: Relayer[];
  count: number;
}

export function RelayPoolTab() {
  const [pool, setPool] = useState<RelayPoolData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchPool = useCallback(async () => {
    try {
      const res = await fetch(`${NODE_URL}/relay/pool`);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      setPool(data);
      setError(null);
    } catch (e: any) {
      setError(e.message || "Failed to fetch relay pool");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchPool();
    const interval = setInterval(fetchPool, 10000);
    return () => clearInterval(interval);
  }, [fetchPool]);

  const formatTime = (timestamp: number) => {
    if (!timestamp) return "never";
    const tsMs = timestamp < 1e12 ? timestamp * 1000 : timestamp;
    const date = new Date(tsMs);
    const now = Date.now();
    const diff = now - tsMs;
    if (diff < 60000) return `${Math.floor(diff / 1000)}s ago`;
    if (diff < 3600000) return `${Math.floor(diff / 60000)}m ago`;
    if (diff < 86400000) return `${Math.floor(diff / 3600000)}h ago`;
    return date.toLocaleDateString();
  };

  const truncateAddr = (addr: string) => {
    if (addr.length <= 16) return addr;
    return addr.slice(0, 8) + "...";
  };

  const truncateUrl = (url: string) => {
    try {
      const u = new URL(url);
      return u.hostname;
    } catch {
      return url.length > 30 ? url.slice(0, 27) + "..." : url;
    }
  };

  if (loading) {
    return (
      <div className="relay-pool-tab">
        <div className="section">
          <h3>relay pool</h3>
          <p style={{ color: "#888", fontSize: 13 }}>loading...</p>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="relay-pool-tab">
        <div className="section">
          <h3>relay pool</h3>
          <p className="error">{error}</p>
          <button onClick={fetchPool} style={{ marginTop: 8 }}>
            retry
          </button>
        </div>
      </div>
    );
  }

  const relayers = pool?.relayers || [];
  const healthyCount = relayers.filter((r) => r.isHealthy).length;
  const totalRelays = relayers.reduce((sum, r) => sum + r.relaysCompleted, 0);
  const totalStake = relayers.reduce((sum, r) => sum + r.stake, 0);
  const avgFee =
    relayers.length > 0
      ? relayers.reduce((sum, r) => sum + r.feeRate, 0) / relayers.length
      : 0;

  return (
    <div className="relay-pool-tab">
      <div className="section">
        <h3>relay pool overview</h3>
        <div className="relay-stats-grid">
          <div className="relay-stat-card">
            <div className="relay-stat-value">{relayers.length}</div>
            <div className="relay-stat-label">total relayers</div>
          </div>
          <div className="relay-stat-card">
            <div className="relay-stat-value healthy">{healthyCount}</div>
            <div className="relay-stat-label">healthy</div>
          </div>
          <div className="relay-stat-card">
            <div className="relay-stat-value">
              {totalRelays.toLocaleString()}
            </div>
            <div className="relay-stat-label">total relays</div>
          </div>
          <div className="relay-stat-card">
            <div className="relay-stat-value">{totalStake.toFixed(2)}</div>
            <div className="relay-stat-label">total stake (RKU)</div>
          </div>
          <div className="relay-stat-card">
            <div className="relay-stat-value">{avgFee.toFixed(4)}</div>
            <div className="relay-stat-label">avg fee rate</div>
          </div>
        </div>
      </div>

      <div className="section">
        <h3>active relayers</h3>
        {relayers.length === 0 ? (
          <p style={{ color: "#888", fontSize: 13 }}>
            no relayers registered in the pool
          </p>
        ) : (
          <div className="relay-table-container">
            <table className="relay-table">
              <thead>
                <tr>
                  <th>status</th>
                  <th>address</th>
                  <th>node</th>
                  <th>fee rate</th>
                  <th>stake</th>
                  <th>relays</th>
                  <th>last seen</th>
                </tr>
              </thead>
              <tbody>
                {relayers
                  .sort((a, b) => {
                    if (a.isHealthy !== b.isHealthy)
                      return a.isHealthy ? -1 : 1;
                    return b.relaysCompleted - a.relaysCompleted;
                  })
                  .map((r) => (
                    <tr
                      key={r.address}
                      className={r.isHealthy ? "" : "unhealthy"}
                    >
                      <td
                        style={{
                          display: "flex",
                          alignContent: "center",
                          alignItems: "center",
                          gap: 10,
                        }}
                      >
                        <span
                          className={`status-dot ${r.isHealthy ? "connected" : "unhealthy"}`}
                        />
                        {r.isHealthy ? " healthy" : " offline"}
                      </td>
                      <td>
                        <a
                          href={`?tab=accounts&address=${r.address}`}
                          onClick={(e) => {
                            e.preventDefault();
                            window.location.href = `/account/${r.address}`;
                          }}
                          className="relay-address"
                          title={r.address}
                        >
                          {truncateAddr(r.address)}
                        </a>
                      </td>
                      <td>
                        <a
                          href={r.nodeUrl}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="relay-node-url"
                          title={r.nodeUrl}
                        >
                          {truncateUrl(r.nodeUrl)}
                        </a>
                      </td>
                      <td className="mono">{r.feeRate.toFixed(4)}</td>
                      <td className="mono">{r.stake.toFixed(2)} RKU</td>
                      <td className="mono">
                        {r.relaysCompleted.toLocaleString()}
                      </td>
                      <td className="relay-last-seen">
                        {formatTime(r.lastSeen)}
                      </td>
                    </tr>
                  ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {/* <div className="section">
        <h3>about meta-transaction relaying</h3>
        <div className="relay-info">
          <p>
            relayers submit transactions on behalf of users, paying gas fees
            upfront. users sign a relay intent off-chain and the relayer wraps
            it in a real transaction. the relayer earns a fee for each relay.
          </p>
          <div className="relay-info-grid">
            <div className="relay-info-item">
              <span className="relay-info-label">privacy</span>
              <span className="relay-info-value">
                sender identity obscured on-chain
              </span>
            </div>
            <div className="relay-info-item">
              <span className="relay-info-label">gasless</span>
              <span className="relay-info-value">
                users don't need RKU for gas
              </span>
            </div>
            <div className="relay-info-item">
              <span className="relay-info-label">censorship resistant</span>
              <span className="relay-info-value">
                multiple relayers prevent single-point blocking
              </span>
            </div>
          </div>
        </div>
      </div> */}
    </div>
  );
}
