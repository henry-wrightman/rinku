import { useState, useEffect, useCallback } from "react";
import type { State, DAGNode } from "./types";
import {
  Header,
  DAGTab,
  AccountsTab,
  FaucetTab,
  ContractsTab,
  RewardsTab,
} from "./components";
import { formatNumber, formatTps } from "./utils";

const NODE_URL = "/api";
const PAGE_SIZE = 20;

interface NetworkStats {
  tps: number;
  finalizedCount: number;
  unfinalizedCount: number;
  finalityRatio: number;
  checkpointCount: number;
  latestCheckpointHeight: number;
  latestCheckpointId: string | null;
  totalStaked: number;
  validatorCount: number;
  networkAge: number;
}

interface GasStats {
  current: number;
  min: number;
  max: number;
  avgLast100: number;
  totalBurned: number;
}

function App() {
  const [tab, setTab] = useState<
    "dag" | "accounts" | "faucet" | "contracts" | "rewards"
  >("dag");
  const [nodes, setNodes] = useState<DAGNode[]>([]);
  const [accounts, setAccounts] = useState<State["accounts"]>([]);
  const [summary, setSummary] = useState<{
    totalNodes: number;
    tipCount: number;
    tips: string[];
    merkleRoot: string;
    accountCount: number;
  } | null>(null);
  const [networkStats, setNetworkStats] = useState<NetworkStats | null>(null);
  const [gasStats, setGasStats] = useState<GasStats | null>(null);
  const [loading, setLoading] = useState(true);
  const [connected, setConnected] = useState(false);
  const [darkMode, setDarkMode] = useState(true);
  const [page, setPage] = useState(0);
  const [hasMore, setHasMore] = useState(false);

  useEffect(() => {
    document.body.classList.toggle("light", !darkMode);
  }, [darkMode]);

  const fetchSummary = useCallback(async () => {
    try {
      const [summaryRes, accountsRes, networkRes, gasPriceRes, gasStatsRes] =
        await Promise.all([
          fetch(`${NODE_URL}/dag/summary`),
          fetch(`${NODE_URL}/accounts`),
          fetch(`${NODE_URL}/stats/network`),
          fetch(`${NODE_URL}/gas/price`),
          fetch(`${NODE_URL}/gas/stats`),
        ]);

      const summaryData = await summaryRes.json();
      const accountsData = await accountsRes.json();

      setSummary(summaryData);
      setAccounts(accountsData.accounts);
      setConnected(true);

      if (networkRes.ok) {
        const networkData = await networkRes.json();
        setNetworkStats(networkData);
      }

      if (gasPriceRes.ok && gasStatsRes.ok) {
        const gasPriceData = await gasPriceRes.json();
        const gasStatsData = await gasStatsRes.json();
        setGasStats({
          current: gasPriceData.current,
          min: gasPriceData.min,
          max: gasPriceData.max,
          avgLast100: gasPriceData.avgLast100,
          totalBurned: gasStatsData.totalBurned,
        });
      }
    } catch (e) {
      console.error("Failed to fetch summary:", e);
      setConnected(false);
    }
  }, []);

  const fetchPage = useCallback(async (pageNum: number) => {
    try {
      const res = await fetch(
        `${NODE_URL}/dag?page=${pageNum}&limit=${PAGE_SIZE}`,
      );
      const data = await res.json();
      setNodes(data.nodes);
      setHasMore(data.hasMore);
      setConnected(true);
    } catch (e) {
      console.error("Failed to fetch page:", e);
      setConnected(false);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchSummary();
    fetchPage(page);
    const interval = setInterval(() => {
      fetchSummary();
      fetchPage(page);
    }, 5000);
    return () => clearInterval(interval);
  }, [page, fetchSummary, fetchPage]);

  const handlePageChange = (newPage: number) => {
    setPage(newPage);
    fetchPage(newPage);
  };

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
        <div className="stat-item">
          <span className="stat-value">
            {formatNumber(summary?.totalNodes || 0)}
          </span>
          <span className="stat-label">nodes</span>
        </div>
        {/* <div className="stat-item">
          <span className="stat-value">{formatNumber(summary?.accountCount || accounts.length || 0)}</span>
          <span className="stat-label">accounts</span>
        </div> */}
        <div className="stat-item">
          <span className="stat-value">
            {formatTps(networkStats?.tps || 0)}
          </span>
          <span className="stat-label">tps</span>
        </div>
        <div className="stat-item">
          <span className="stat-value">
            {networkStats?.finalityRatio || 0}%
          </span>
          <span className="stat-label">finalized</span>
        </div>
        <div className="stat-item">
          <span className="stat-value">
            {formatNumber(networkStats?.latestCheckpointHeight || 0)}
          </span>
          <span className="stat-label">checkpoint height</span>
        </div>
        <div className="stat-item">
          <span className="stat-value">
            {formatNumber(networkStats?.totalStaked || 0)}
          </span>
          <span className="stat-label">staked</span>
        </div>
        <div className="stat-item">
          <span className="stat-value">
            {networkStats?.validatorCount || 0}
          </span>
          <span className="stat-label">validators</span>
        </div>
        <div className="stat-item">
          <span className="stat-value">
            {gasStats?.current?.toFixed(4) || "0.0100"}
          </span>
          <span className="stat-label">gas price</span>
        </div>
        <div className="stat-item">
          <span className="stat-value">
            {formatNumber(gasStats?.totalBurned || 0)}
          </span>
          <span className="stat-label">burned</span>
        </div>
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
        <span
          className={tab === "contracts" ? "active" : ""}
          onClick={() => setTab("contracts")}
        >
          contracts
        </span>
        <span
          className={tab === "rewards" ? "active" : ""}
          onClick={() => setTab("rewards")}
        >
          rewards
        </span>
      </div>

      {tab === "dag" && (
        <DAGTab
          nodes={nodes}
          merkleRoot={summary?.merkleRoot || ""}
          page={page}
          totalNodes={summary?.totalNodes || 0}
          hasMore={hasMore}
          onPageChange={handlePageChange}
        />
      )}
      {tab === "accounts" && <AccountsTab accounts={accounts} />}
      {tab === "faucet" && (
        <FaucetTab
          onSuccess={() => {
            fetchSummary();
            fetchPage(page);
          }}
        />
      )}
      {tab === "contracts" && <ContractsTab />}
      {tab === "rewards" && <RewardsTab />}
    </div>
  );
}

export default App;
