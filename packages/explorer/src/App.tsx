import { useState, useEffect } from "react";
import type { State } from "./types";
import { Header, DAGTab, AccountsTab, FaucetTab, ContractsTab } from "./components";

const NODE_URL = "/api";

function App() {
  const [tab, setTab] = useState<"dag" | "accounts" | "faucet" | "contracts">("dag");
  const [state, setState] = useState<State | null>(null);
  const [loading, setLoading] = useState(true);
  const [connected, setConnected] = useState(false);
  const [darkMode, setDarkMode] = useState(true);

  useEffect(() => {
    document.body.classList.toggle("light", !darkMode);
  }, [darkMode]);

  const fetchState = async () => {
    try {
      const [dagRes, accountsRes] = await Promise.all([
        fetch(`${NODE_URL}/dag`),
        fetch(`${NODE_URL}/accounts`),
      ]);

      const dagData = (await dagRes.json()) as {
        nodes: State["nodes"];
        tips: string[];
        tipUrls: string[];
        merkleRoot: string;
      };
      const accountsData = (await accountsRes.json()) as {
        accounts: State["accounts"];
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
      console.error("Failed to fetch state:", e);
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

  if (loading) {
    return (
      <div className="container">
        <Header connected={false} />
        <div className="loading">loading...</div>
      </div>
    );
  }

  return (
    <div className="container">
      <Header connected={connected} />

      <button className="theme-toggle" onClick={() => setDarkMode(!darkMode)}>
        {darkMode ? "☀" : "☾"}
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
        <span className={tab === "dag" ? "active" : ""} onClick={() => setTab("dag")}>
          dag
        </span>
        <span className={tab === "accounts" ? "active" : ""} onClick={() => setTab("accounts")}>
          accounts
        </span>
        <span className={tab === "faucet" ? "active" : ""} onClick={() => setTab("faucet")}>
          faucet
        </span>
        <span className={tab === "contracts" ? "active" : ""} onClick={() => setTab("contracts")}>
          contracts
        </span>
      </div>

      {tab === "dag" && state && (
        <DAGTab nodes={state.nodes} merkleRoot={state.merkleRoot} />
      )}

      {tab === "accounts" && state && <AccountsTab accounts={state.accounts} />}

      {tab === "faucet" && <FaucetTab onSuccess={fetchState} />}

      {tab === "contracts" && <ContractsTab />}
    </div>
  );
}

export default App;
