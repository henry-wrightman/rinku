import { useState, useEffect, useCallback, useRef } from "react";
import { useSearchParams } from "react-router-dom";
import type { State, DAGNode } from "./types";
import { useWebSocketContext } from "./context/WebSocketContext";
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
  ThreadTab,
  AnimatedNumber,
  ChatTab,
  ChatRoomsTab,
} from "./components";
import { formatNumber, formatTps } from "./utils";
import { useTheme } from "./hooks/useTheme";
import { useRinku } from "./context/WalletContext";
import { API_URL } from "./config";

const NODE_URL = API_URL;
const PAGE_SIZE = 50;

export interface P2pStats {
  peers: Peer[];
  peerCount: number;
}

export interface Peer {
  peer_id: string;
  connected_at: number;
  messages_received: number;
  messages_sent: number;
  last_seen: number;
  handshake_validated: boolean;
  handshake_info: HandshakeInfo;
  rate_limit_tokens: number;
  last_rate_update: number;
  score: number;
}

export interface HandshakeInfo {
  protocol_version: string;
  chain_id: string;
  network_id: string;
  node_id: string;
  checkpoint_height: number;
  validator_address: any;
  capabilities: string[];
}

export interface HandshakeInfo {
  protocol_version: string;
  chain_id: string;
  network_id: string;
  node_id: string;
  checkpoint_height: number;
  validator_address: any;
  capabilities: string[];
}

interface NetworkStats {
  tps: number;
  tpsShort: number;
  tpsLong: number;
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
  avgConfirmationMs: number | null;
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
  | "verify"
  | "thread"
  | "chat"
  | "rooms";

const validTabs: TabType[] = [
  "dag",
  "accounts",
  "faucet",
  "contracts",
  "rewards",
  "tokenomics",
  "zk",
  "verify",
  "thread",
  "chat",
  "rooms",
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
  const smoothedTpsRef = useRef<{ tps: number; short: number; long: number }>({
    tps: -1,
    short: -1,
    long: -1,
  });
  const [gasStats, setGasStats] = useState<GasStats | null>(null);
  const [finalityStats, setFinalityStats] = useState<FinalityStats | null>(
    null,
  );
  const smoothedConfirmationRef = useRef<number | null>(null);
  const [peerStats, setPeerStats] = useState<P2pStats | null>(null);
  const [loading, setLoading] = useState(true);
  const [connected, setConnected] = useState(false);
  const { darkMode, toggleTheme } = useTheme();
  const [walletOpen, setWalletOpen] = useState(false);
  const [aboutOpen, setAboutOpen] = useState(false);
  const { wallet } = useRinku();

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
        peerStatsRes,
      ] = await Promise.all([
        fetch(`${NODE_URL}/dag/summary`),
        fetch(`${NODE_URL}/accounts`),
        fetch(`${NODE_URL}/network/stats`),
        fetch(`${NODE_URL}/gas/price`),
        fetch(`${NODE_URL}/gas/stats`),
        fetch(`${NODE_URL}/finality/metrics`),
        fetch(`${NODE_URL}/peers`),
      ]);

      const summaryData = await summaryRes.json();
      const accountsData = await accountsRes.json();

      setSummary(summaryData);
      setAccounts(accountsData.accounts);
      setConnected(true);

      if (networkRes.ok) {
        const networkData = await networkRes.json();
        const alpha = 0.3;
        const prev = smoothedTpsRef.current;
        const smoothed = {
          tps:
            prev.tps < 0
              ? networkData.tps
              : prev.tps + alpha * (networkData.tps - prev.tps),
          short:
            prev.short < 0
              ? networkData.tpsShort
              : prev.short + alpha * (networkData.tpsShort - prev.short),
          long:
            prev.long < 0
              ? networkData.tpsLong
              : prev.long + alpha * (networkData.tpsLong - prev.long),
        };
        smoothedTpsRef.current = smoothed;
        setNetworkStats({
          ...networkData,
          tps: smoothed.tps,
          tpsShort: smoothed.short,
          tpsLong: smoothed.long,
        });
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
        const rawMs = finalityData.avgConfirmationMs;
        const prev = smoothedConfirmationRef.current;
        if (rawMs != null && rawMs > 0) {
          const smoothed =
            prev == null ? rawMs : Math.round(prev + 0.3 * (rawMs - prev));
          smoothedConfirmationRef.current = smoothed;
          finalityData.avgConfirmationMs = smoothed;
        } else if (prev != null) {
          finalityData.avgConfirmationMs = prev;
        }
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

  const { status: wsStatus, lastBatch } = useWebSocketContext();
  const lastBatchIdRef = useRef(0);
  const refreshTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pageRef = useRef(page);
  pageRef.current = page;

  useEffect(() => {
    fetchSummary();
    fetchPage(page);
  }, [page, fetchSummary, fetchPage]);

  useEffect(() => {
    if (!lastBatch || lastBatch.id === lastBatchIdRef.current) return;
    lastBatchIdRef.current = lastBatch.id;
    if (refreshTimerRef.current) return;
    refreshTimerRef.current = setTimeout(() => {
      refreshTimerRef.current = null;
      fetchSummary();
      fetchPage(pageRef.current);
    }, 250);
  }, [lastBatch, fetchSummary, fetchPage]);

  useEffect(() => {
    return () => {
      if (refreshTimerRef.current) {
        clearTimeout(refreshTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (wsStatus === "connected") return;
    const interval = setInterval(() => {
      fetchSummary();
      fetchPage(page);
    }, 5000);
    return () => clearInterval(interval);
  }, [wsStatus, page, fetchSummary, fetchPage]);

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
          wsStatus={wsStatus}
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
        wsStatus={wsStatus}
        peersConnected={peerStats?.peerCount}
      />

      <div className="header-actions">
        <button className="about-link" onClick={() => setAboutOpen(true)}>
          about
        </button>
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

      <WalletModal isOpen={walletOpen} onClose={() => setWalletOpen(false)} />

      {aboutOpen && (
        <div className="about-overlay" onClick={() => setAboutOpen(false)}>
          <div className="about-modal" onClick={(e) => e.stopPropagation()}>
            <button className="about-close" onClick={() => setAboutOpen(false)}>
              ×
            </button>
            <h3>about rinku</h3>
            <p>
              rinku is an experimental project and a work in progress. even when
              it eventually launches to mainnet, it'll likely not be listed on
              exchanges. simply an open source project to either contribute to,
              or fork &amp; make into something better.
            </p>
          </div>
        </div>
      )}

      <div className="stats-ticker">
        <div className="ticker-row">
          <span
            className="ticker-cell"
            title={
              networkStats
                ? `15s: ${formatTps(networkStats.tpsShort)} · 60s: ${formatTps(networkStats.tpsLong)}`
                : ""
            }
          >
            <span className="tv">
              {formatNumber(networkStats?.totalTransactionsProcessed || 0)}
            </span>
            <span className="tl">tx</span>
            <span className="sep" />
            <span className="tv accent">
              {formatTps(networkStats?.tps || 0)}
            </span>
            <span className="tl">tps</span>
          </span>
          <span className="ticker-cell">
            {/* <span className="tv">
              {((networkStats?.finalityRatio || 0) * 100).toFixed(0)}%
            </span>
            <span className="tl">final</span>
            <span className="sep" /> */}
            <span className="tv">
              {formatNumber(networkStats?.latestCheckpointHeight || 0)}
            </span>
            <span className="tl">height</span>
          </span>
          <span className="ticker-cell">
            <span className="tv">
              {formatNumber(networkStats?.totalStaked || 0)}
            </span>
            <span className="tl">staked</span>
            <span className="sep" />
            <span className="tv">{networkStats?.validatorCount || 0}</span>
            <span className="tl">validators</span>
          </span>
          <span className="ticker-cell">
            <span className="tv warm">
              {gasStats?.current?.toFixed(4) || "0.0100"}
            </span>
            <span className="tl">gas</span>
            <span className="sep" />
            <span className="tv warm">
              {formatNumber(gasStats?.totalBurned || 0, 2)}
            </span>
            <span className="tl">burned</span>
          </span>
        </div>
        {finalityStats &&
          (finalityStats.avgTimeToFinality > 0 ||
            finalityStats.pendingCount > 0 ||
            finalityStats.avgConfirmationMs != null) && (
            <div className="ticker-row secondary">
              <span className="ticker-cell">
                <span className="tv cyan">
                  {finalityStats.avgConfirmationMs != null
                    ? `${finalityStats.avgConfirmationMs}ms`
                    : "-"}
                </span>
                <span className="tl">convergence</span>
              </span>
              <span className="ticker-cell">
                <span className="tv cyan">
                  {finalityStats.checkpointsPerMinute > 0
                    ? `${(60 / finalityStats.checkpointsPerMinute).toFixed(1)}s`
                    : "-"}
                </span>
                <span className="tl">snapshot</span>
              </span>
              <span className="ticker-cell">
                <span className="tv cyan">{finalityStats.pendingCount}</span>
                <span className="tl">pending</span>
              </span>
              <span className="ticker-cell">
                <span className="tv cyan">
                  {finalityStats.checkpointsPerMinute.toFixed(1)}/min
                </span>
                <span className="tl">cp rate</span>
                <span className="sep" />
                <span className="tv cyan">
                  {(finalityStats.lastCheckpointAge / 1000).toFixed(0)}s
                </span>
                <span className="tl">ago</span>
              </span>
            </div>
          )}
      </div>

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
          <span className="hamburger-icon">{mobileNavOpen ? "✕" : "☰"}</span>
          <span className="current-tab">{tab}</span>
        </button>
        <div className={`nav ${mobileNavOpen ? "open" : ""}`}>
          <span
            className={tab === "dag" ? "active" : ""}
            onClick={() => {
              setTab("dag");
              setMobileNavOpen(false);
            }}
          >
            dag
          </span>
          <span
            className={tab === "accounts" ? "active" : ""}
            onClick={() => {
              setTab("accounts");
              setMobileNavOpen(false);
            }}
          >
            accounts
          </span>
          <span
            className={tab === "faucet" ? "active" : ""}
            onClick={() => {
              setTab("faucet");
              setMobileNavOpen(false);
            }}
          >
            faucet
          </span>
          <span
            className={tab === "contracts" ? "active" : ""}
            onClick={() => {
              setTab("contracts");
              setMobileNavOpen(false);
            }}
          >
            contracts
          </span>
          {/* <span
            className={tab === "zk" ? "active" : ""}
            onClick={() => {
              setTab("zk");
              setMobileNavOpen(false);
            }}
          >
            zk
          </span> */}
          <span
            className={tab === "rewards" ? "active" : ""}
            onClick={() => {
              setTab("rewards");
              setMobileNavOpen(false);
            }}
          >
            rewards
          </span>
          {/* <span
            className={tab === "tokenomics" ? "active" : ""}
            onClick={() => {
              setTab("tokenomics");
              setMobileNavOpen(false);
            }}
          >
            tokenomics
          </span> */}
          <span
            className={tab === "verify" ? "active" : ""}
            onClick={() => {
              setTab("verify");
              setMobileNavOpen(false);
            }}
          >
            verify
          </span>
          {/* <span
            className={tab === "thread" ? "active" : ""}
            onClick={() => {
              setTab("thread");
              setMobileNavOpen(false);
            }}
          >
            thread
          </span> */}
          {/* <span
            className={tab === "chat" ? "active" : ""}
            onClick={() => {
              setTab("chat");
              setMobileNavOpen(false);
            }}
          >
            chat
          </span> */}
          <span
            className={tab === "rooms" ? "active" : ""}
            onClick={() => {
              setTab("rooms");
              setMobileNavOpen(false);
            }}
          >
            rooms
          </span>
        </div>
      </div>
      {mobileNavOpen && (
        <div className="nav-overlay" onClick={() => setMobileNavOpen(false)} />
      )}

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
      {tab === "thread" && (
        <ThreadTab onWalletOpen={() => setWalletOpen(true)} />
      )}
      {tab === "chat" && <ChatTab onWalletOpen={() => setWalletOpen(true)} />}
      {tab === "rooms" && (
        <ChatRoomsTab onWalletOpen={() => setWalletOpen(true)} />
      )}
      {/* <div className="">
        <span style={{ fontSize: 10 }}>
          rinku is an experimental work-in-progress 🫡
        </span>
      </div> */}
    </div>
  );
}

export default App;
