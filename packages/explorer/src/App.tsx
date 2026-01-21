import { useState, useEffect, useCallback } from "react";
import { useSearchParams } from "react-router-dom";
import type { State, DAGNode } from "./types";
import {
  Header,
  DAGTab,
  AccountsTab,
  FaucetTab,
  ContractsTab,
  RewardsTab,
  TokenomicsTab,
  SearchBar,
  ZKTab,
  VerifyProofTab,
  WalletModal,
} from "./components";
import {
  deserializeKeyPair,
  validateSerializedKey,
  type SerializedKeyPair,
} from "./crypto";
import { formatNumber, formatTps } from "./utils";
import { useTheme } from "./hooks/useTheme";

// In production, API is on same host but port 3001. In dev, Vite proxies /api to 3001.
const getApiBaseUrl = () => {
  if (import.meta.env.PROD) {
    // Production: construct API URL from current host
    const host = window.location.hostname;
    console.log(
      "prod api url",
      `https://${host.replace(/-5000\./, "-3001.")}/api`,
    );
    return `https://${host.replace(/-5000\./, "-3001.")}/api`;
  }
  return "/api"; // Dev: use Vite proxy
};
const NODE_URL = getApiBaseUrl();
const PAGE_SIZE = 20;

interface NetworkStats {
  tps: number;
  totalTransactionsProcessed: number;
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

interface PeerStats {
  peersConnected: number;
  messagesSent: number;
  messagesReceived: number;
  lastGossipAt: number;
}

interface FinalityStats {
  avgTimeToFinality: number;
  medianTimeToFinality: number;
  p95TimeToFinality: number;
  pendingCount: number;
  finalizedCount: number;
  finalityRate: number;
  checkpointLatency: number;
  checkpointsPerMinute: number;
  lastCheckpointAge: number;
  txThroughput: number;
}

interface VersionInfo {
  protocolVersion: string;
  nodeVersion: string;
  chainId: string;
  networkId: string;
  features: { id: string; name: string; status: string }[];
}

type TabType =
  | "dag"
  | "accounts"
  | "faucet"
  | "contracts"
  | "rewards"
  | "tokenomics"
  | "zk"
  | "verify";

const validTabs: TabType[] = [
  "dag",
  "accounts",
  "faucet",
  "contracts",
  "rewards",
  "tokenomics",
  "zk",
  "verify",
];

function App() {
  const [searchParams, setSearchParams] = useSearchParams();
  const tabParam = searchParams.get("tab");
  const initialTab =
    tabParam && validTabs.includes(tabParam as TabType)
      ? (tabParam as TabType)
      : "dag";
  const [tab, setTabState] = useState<TabType>(initialTab);
  const [mobileNavOpen, setMobileNavOpen] = useState(false);

  const setTab = useCallback(
    (newTab: TabType) => {
      setTabState(newTab);
      if (newTab === "dag") {
        setSearchParams({});
      } else {
        setSearchParams({ tab: newTab });
      }
    },
    [setSearchParams],
  );

  // Sync tab state when URL changes (e.g., back/forward navigation)
  useEffect(() => {
    const urlTab = searchParams.get("tab");
    const newTab =
      urlTab && validTabs.includes(urlTab as TabType)
        ? (urlTab as TabType)
        : "dag";
    if (newTab !== tab) {
      setTabState(newTab);
    }
  }, [searchParams, tab]);

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
  const [finalityStats, setFinalityStats] = useState<FinalityStats | null>(
    null,
  );
  const [peerStats, setPeerStats] = useState<PeerStats | null>(null);
  const [loading, setLoading] = useState(true);
  const [connected, setConnected] = useState(false);
  const { darkMode, toggleTheme } = useTheme();
  const [walletOpen, setWalletOpen] = useState(false);
  const [wallet, setWallet] = useState<SerializedKeyPair | null>(null);

  useEffect(() => {
    const stored = localStorage.getItem("rinku_wallet");
    if (stored && validateSerializedKey(stored)) {
      try {
        setWallet(deserializeKeyPair(stored));
      } catch (e) {
        console.error("Failed to load stored wallet:", e);
      }
    }
  }, []);
  const [page, setPage] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [searchResult, setSearchResult] = useState<{
    type: "transaction" | "account" | "contract" | null;
    data: any;
    error?: string;
  } | null>(null);
  const [versionInfo, setVersionInfo] = useState<VersionInfo | null>(null);

  const fetchSummary = useCallback(async () => {
    try {
      const [
        summaryRes,
        accountsRes,
        networkRes,
        gasPriceRes,
        gasStatsRes,
        finalityRes,
        peerStatsRes
      ] = await Promise.all([
        fetch(`${NODE_URL}/dag/summary`),
        fetch(`${NODE_URL}/accounts`),
        fetch(`${NODE_URL}/network/stats`),
        fetch(`${NODE_URL}/gas/price`),
        fetch(`${NODE_URL}/gas/stats`),
        fetch(`${NODE_URL}/finality/metrics`),
        fetch(`${NODE_URL}/gossip/stats`),
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

      if (finalityRes.ok) {
        const finalityData = await finalityRes.json();
        setFinalityStats(finalityData);
      }

      if (peerStatsRes.ok) {
        const peerData = await peerStatsRes.json();
        setPeerStats(peerData);
      }

      const versionRes = await fetch(`${NODE_URL}/version`);
      if (versionRes.ok) {
        const versionData = await versionRes.json();
        setVersionInfo(versionData);
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
        <Header
          connected={false}
          protocolVersion={versionInfo?.protocolVersion}
          nodeVersion={versionInfo?.nodeVersion}
        />
        <div className="loading">loading...</div>
      </div>
    );
  }

  return (
    <div className="container">
      <Header
        connected={connected}
        protocolVersion={versionInfo?.protocolVersion}
        nodeVersion={versionInfo?.nodeVersion}
        peersConnected={peerStats?.peersConnected || 0}
      />

      <div className="header-actions">
        <a target="_blank" className="whitepaper-link" href="/rinku.pdf">
          whitepaper
        </a>
        <button
          className={`wallet-btn-header ${wallet ? "connected" : ""}`}
          onClick={() => setWalletOpen(true)}
        >
          {wallet ? `${wallet.fingerprint.slice(0, 6)}...` : "wallet"}
        </button>
        <button className="theme-toggle" onClick={toggleTheme}>
          {darkMode ? "☀" : "☾"}
        </button>
      </div>

      <WalletModal
        isOpen={walletOpen}
        onClose={() => setWalletOpen(false)}
        onWalletChange={setWallet}
      />

      <div className="stats">
        <div className="stat-item">
          <span className="stat-value">
            {formatNumber(networkStats?.totalTransactionsProcessed || 0)}
          </span>
          <span className="stat-label">transactions</span>
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
            {((networkStats?.finalityRatio || 0) * 100).toFixed(0)}%
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
            {formatNumber(gasStats?.totalBurned || 0, 2)}
          </span>
          <span className="stat-label">burned</span>
        </div>
      </div>

      {finalityStats &&
        (finalityStats.avgTimeToFinality > 0 ||
          finalityStats.pendingCount > 0) && (
          <div className="stats finality-stats">
            <div className="stat-item">
              <span className="stat-value">
                {(finalityStats.avgTimeToFinality / 1000).toFixed(1)}s
              </span>
              <span className="stat-label">avg finality</span>
            </div>
            <div className="stat-item">
              <span className="stat-value">
                {(finalityStats.p95TimeToFinality / 1000).toFixed(1)}s
              </span>
              <span className="stat-label">p95 finality</span>
            </div>
            <div className="stat-item">
              <span className="stat-value">{finalityStats.pendingCount}</span>
              <span className="stat-label">pending</span>
            </div>
            <div className="stat-item">
              <span className="stat-value">
                {finalityStats.checkpointsPerMinute.toFixed(1)}/min
              </span>
              <span className="stat-label">checkpoints</span>
            </div>
            <div className="stat-item">
              <span className="stat-value">
                {(finalityStats.lastCheckpointAge / 1000).toFixed(0)}s
              </span>
              <span className="stat-label">last checkpoint</span>
            </div>
          </div>
        )}

      <SearchBar onResult={setSearchResult} />

      {searchResult && (
        <div className="search-result-modal">
          <div className="search-result-content">
            <button className="close-btn" onClick={() => setSearchResult(null)}>
              x
            </button>
            {searchResult.error ? (
              <div className="error">{searchResult.error}</div>
            ) : (
              <>
                <h3>{searchResult.type}</h3>
                <pre>{JSON.stringify(searchResult.data, null, 2)}</pre>
              </>
            )}
          </div>
        </div>
      )}

      <div className="nav-container">
        <button 
          className="mobile-nav-toggle" 
          onClick={() => setMobileNavOpen(!mobileNavOpen)}
          aria-label="Toggle navigation"
        >
          <span className="hamburger-icon">{mobileNavOpen ? '✕' : '☰'}</span>
          <span className="current-tab">{tab}</span>
        </button>
        <div className={`nav ${mobileNavOpen ? 'open' : ''}`}>
          <span
            className={tab === "dag" ? "active" : ""}
            onClick={() => { setTab("dag"); setMobileNavOpen(false); }}
          >
            dag
          </span>
          <span
            className={tab === "accounts" ? "active" : ""}
            onClick={() => { setTab("accounts"); setMobileNavOpen(false); }}
          >
            accounts
          </span>
          <span
            className={tab === "faucet" ? "active" : ""}
            onClick={() => { setTab("faucet"); setMobileNavOpen(false); }}
          >
            faucet
          </span>
          <span
            className={tab === "contracts" ? "active" : ""}
            onClick={() => { setTab("contracts"); setMobileNavOpen(false); }}
          >
            contracts
          </span>
          <span
            className={tab === "zk" ? "active" : ""}
            onClick={() => { setTab("zk"); setMobileNavOpen(false); }}
          >
            zk
          </span>
          <span
            className={tab === "rewards" ? "active" : ""}
            onClick={() => { setTab("rewards"); setMobileNavOpen(false); }}
          >
            rewards
          </span>
          <span
            className={tab === "tokenomics" ? "active" : ""}
            onClick={() => { setTab("tokenomics"); setMobileNavOpen(false); }}
          >
            tokenomics
          </span>
          <span
            className={tab === "verify" ? "active" : ""}
            onClick={() => { setTab("verify"); setMobileNavOpen(false); }}
          >
            verify
          </span>
        </div>
      </div>
      {mobileNavOpen && <div className="nav-overlay" onClick={() => setMobileNavOpen(false)} />}

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
      {tab === "tokenomics" && <TokenomicsTab />}
      {tab === "zk" && <ZKTab />}
      {tab === "verify" && <VerifyProofTab />}
    </div>
  );
}

export default App;
