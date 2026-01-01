import { useState } from "react";

interface FaucetTabProps {
  onSuccess: () => void;
}

export function FaucetTab({ onSuccess }: FaucetTabProps) {
  const [address, setAddress] = useState("");
  const [message, setMessage] = useState<{ type: "success" | "error"; text: string } | null>(null);

  const requestFaucet = async () => {
    if (!address) {
      setMessage({ type: "error", text: "address required" });
      return;
    }

    try {
      const res = await fetch("/api/faucet/request", {
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
          text: `received ${data.amount} coins. tx: ${data.txHash.slice(0, 8)}...`,
        });
        onSuccess();
      } else {
        setMessage({ type: "error", text: data.error || "request failed" });
      }
    } catch {
      setMessage({ type: "error", text: "failed to connect to faucet" });
    }
  };

  return (
    <div className="section">
      <div className="hint">get testnet coins. rate limited to once per minute.</div>

      {message && <div className={`message ${message.type}`}>{message.text}</div>}

      <input
        type="text"
        placeholder="paste your address (fingerprint)"
        value={address}
        onChange={(e) => setAddress(e.target.value)}
      />

      <button className="btn" onClick={requestFaucet}>
        request coins
      </button>

      <div style={{ marginTop: 24, color: "#444", fontSize: 12 }}>
        tip: use @rinku/wallet to generate an address
      </div>
    </div>
  );
}
