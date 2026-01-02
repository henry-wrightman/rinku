import { useState, useEffect } from "react";

interface FaucetStats {
  rateLimitEntries: number;
  maxEntries: number;
  nodeUrl: string;
}

interface FaucetTabProps {
  onSuccess: () => void;
}

const FAUCET_URL = "/api/faucet";

export function FaucetTab({ onSuccess }: FaucetTabProps) {
  const [address, setAddress] = useState("");
  const [message, setMessage] = useState<{ type: "success" | "error"; text: string } | null>(null);
  const [loading, setLoading] = useState(false);
  const [stats, setStats] = useState<FaucetStats | null>(null);
  const [recentDrops, setRecentDrops] = useState<{ address: string; amount: number; time: number }[]>([]);

  const fetchStats = async () => {
    try {
      const res = await fetch(`${FAUCET_URL}/stats`);
      const data = await res.json();
      setStats(data);
    } catch (e) {
      console.error("Failed to fetch faucet stats:", e);
    }
  };

  useEffect(() => {
    fetchStats();
    const interval = setInterval(fetchStats, 10000);
    return () => clearInterval(interval);
  }, []);

  const requestFaucet = async () => {
    if (!address) {
      setMessage({ type: "error", text: "address required" });
      return;
    }

    setLoading(true);
    setMessage(null);

    try {
      const res = await fetch(`${FAUCET_URL}/request`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ address }),
      });

      const data = (await res.json()) as {
        amount?: number;
        txHash?: string;
        error?: string;
      };

      if (res.ok && data.amount && data.txHash) {
        setMessage({
          type: "success",
          text: `received ${data.amount} coins`,
        });
        setRecentDrops(prev => [
          { address: address.slice(0, 12) + "...", amount: data.amount!, time: Date.now() },
          ...prev.slice(0, 4)
        ]);
        onSuccess();
        fetchStats();
      } else {
        setMessage({ type: "error", text: data.error || "request failed" });
      }
    } catch {
      setMessage({ type: "error", text: "failed to connect to faucet" });
    } finally {
      setLoading(false);
    }
  };

  const formatTime = (ts: number) => {
    return new Date(ts).toLocaleTimeString();
  };

  return (
    <div className="rewards-tab">
      <div className="section">
        <h3>faucet status</h3>
        <div className="staking-overview">
          <div className="stat-row">
            <span>drop amount:</span>
            <span className="value">100 coins</span>
          </div>
          <div className="stat-row">
            <span>rate limit:</span>
            <span className="value">1 request / 60s</span>
          </div>
          {stats && (
            <>
              <div className="stat-row">
                <span>active rate limits:</span>
                <span className="value">{stats.rateLimitEntries}</span>
              </div>
            </>
          )}
        </div>
      </div>

      <div className="section">
        <h3>request coins</h3>
        <div className="form-row">
          <input
            type="text"
            placeholder="your wallet address (fingerprint)"
            value={address}
            onChange={(e) => setAddress(e.target.value)}
          />
          <button onClick={requestFaucet} disabled={!address || loading}>
            {loading ? "requesting..." : "request"}
          </button>
        </div>

        {message && (
          <div className={message.type === "success" ? "success" : "error"}>
            {message.text}
          </div>
        )}

        <div className="hint" style={{ marginTop: 16, fontSize: 11, color: "#555" }}>
          tip: use @rinku/wallet to generate an address
        </div>
      </div>

      {recentDrops.length > 0 && (
        <div className="section">
          <h3>recent drops</h3>
          <div className="reward-history">
            {recentDrops.map((drop, i) => (
              <div key={i} className="history-row">
                <span className="type mono">{drop.address}</span>
                <span className="amount">+{drop.amount}</span>
                <span className="time">{formatTime(drop.time)}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
