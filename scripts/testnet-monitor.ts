#!/usr/bin/env npx tsx
import WebSocket from "ws";
import * as fs from "fs";
import * as path from "path";

const nodes: string[] = process.argv.slice(2);
if (nodes.length === 0) {
  console.log(
    "Usage: npx tsx scripts/testnet-monitor.ts NODE1_URL [NODE2_URL ...]"
  );
  console.log(
    "  npx tsx scripts/testnet-monitor.ts https://rinku-validator-2.fly.dev https://rinku-validator-1.fly.dev https://rinku-genesis.fly.dev"
  );
  process.exit(1);
}

const LOG_DIR = path.join(process.cwd(), "logs");
if (!fs.existsSync(LOG_DIR)) fs.mkdirSync(LOG_DIR, { recursive: true });

const sessionId = new Date()
  .toISOString()
  .replace(/[:.]/g, "-")
  .replace("T", "_")
  .slice(0, 19);
const JSONL_PATH = path.join(LOG_DIR, `monitor-${sessionId}.jsonl`);
const SUMMARY_PATH = path.join(LOG_DIR, `monitor-${sessionId}.summary.txt`);

const jsonlStream = fs.createWriteStream(JSONL_PATH, { flags: "a" });
const summaryStream = fs.createWriteStream(SUMMARY_PATH, { flags: "a" });

interface NodeTracker {
  url: string;
  name: string;
  wsUrl: string;
  ws: WebSocket | null;
  connected: boolean;
  lastCheckpointHeight: number;
  checkpointMerkleRoots: Map<number, string>;
  txCount: number;
  fastPathCount: number;
  fastPathExecutedCount: number;
  lastEventTime: number;
  reconnectAttempts: number;
  reconnectTimer: ReturnType<typeof setTimeout> | null;
  healthSnapshots: HealthSnapshot[];
}

interface HealthSnapshot {
  ts: number;
  checkpointHeight: number;
  dagSize: number;
  tipCount: number;
  totalTransactions: number;
  validators: number;
  totalStake: number;
  faucetBalance: number;
  peerCount: number;
}

interface CheckpointRecord {
  height: number;
  hash: string;
  txsFinalized: number;
  reward: number;
  merkleRoots: Map<string, string>;
  firstSeen: number;
  seenBy: string[];
  forkAlerted: boolean;
  consensusLogged: boolean;
  confirmScheduled: boolean;
}

const trackers: NodeTracker[] = [];
const checkpointIndex: Map<number, CheckpointRecord> = new Map();
const forkAlerts: { height: number; ts: number; roots: string[] }[] = [];
let globalStartTime = Date.now();
let healthPollInterval: ReturnType<typeof setInterval> | null = null;
let isShuttingDown = false;

const HEALTH_POLL_MS = 10_000;
const RECONNECT_BASE_MS = 2_000;
const RECONNECT_MAX_MS = 30_000;
const STALL_THRESHOLD_MS = 30_000;
const MAX_CHECKPOINT_INDEX = 2000;
const MAX_MERKLE_CACHE = 500;
const MAX_FORK_ALERTS = 500;
// A node's reported merkle root for a height can differ transiently while a
// reorg settles. Only alert on a fork if the divergence is still present after
// re-fetching every node once, this many ms later.
const FORK_CONFIRM_MS = 2_500;

function emit(record: Record<string, unknown>) {
  const line = JSON.stringify({ ...record, _ts: Date.now() });
  jsonlStream.write(line + "\n");
}

function log(msg: string) {
  const ts = new Date().toISOString().slice(11, 23);
  const line = `[${ts}] ${msg}`;
  console.log(line);
  summaryStream.write(line + "\n");
}

function logAlert(msg: string) {
  const ts = new Date().toISOString().slice(11, 23);
  const line = `[${ts}] \x1b[31mALERT\x1b[0m ${msg}`;
  console.log(line);
  summaryStream.write(`[${ts}] ALERT ${msg}\n`);
}

function wsUrlFromHttp(httpUrl: string): string {
  const u = new URL(httpUrl);
  const proto = u.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${u.host}/api/ws`;
}

function initTracker(url: string, index: number): NodeTracker {
  const name =
    url.includes("genesis") ? "genesis" :
    url.includes("validator-1") ? "val-1" :
    url.includes("validator-2") ? "val-2" :
    `node-${index}`;

  return {
    url: url.replace(/\/$/, ""),
    name,
    wsUrl: wsUrlFromHttp(url),
    ws: null,
    connected: false,
    lastCheckpointHeight: 0,
    checkpointMerkleRoots: new Map(),
    txCount: 0,
    fastPathCount: 0,
    fastPathExecutedCount: 0,
    lastEventTime: 0,
    reconnectAttempts: 0,
    reconnectTimer: null,
    healthSnapshots: [],
  };
}

function connectNode(tracker: NodeTracker) {
  if (isShuttingDown) return;

  if (tracker.reconnectTimer) {
    clearTimeout(tracker.reconnectTimer);
    tracker.reconnectTimer = null;
  }

  if (tracker.ws) {
    tracker.ws.removeAllListeners();
    try {
      tracker.ws.terminate();
    } catch {}
    tracker.ws = null;
  }

  log(`Connecting to ${tracker.name} (${tracker.wsUrl})...`);

  const ws = new WebSocket(tracker.wsUrl, {
    handshakeTimeout: 10_000,
    headers: { "User-Agent": "rinku-monitor/1.0" },
  });

  tracker.ws = ws;

  ws.on("open", () => {
    tracker.connected = true;
    tracker.reconnectAttempts = 0;
    tracker.lastEventTime = Date.now();
    log(`${tracker.name} connected`);

    ws.send(JSON.stringify({ subscribe: ["all"] }));

    emit({
      event: "ws_connected",
      node: tracker.name,
      url: tracker.url,
    });
  });

  ws.on("message", (raw: Buffer | string) => {
    const text = raw.toString();
    try {
      const msg = JSON.parse(text);
      tracker.lastEventTime = Date.now();
      handleNodeEvent(tracker, msg);
    } catch {}
  });

  ws.on("close", (code: number) => {
    tracker.connected = false;
    log(`${tracker.name} disconnected (code=${code})`);
    emit({ event: "ws_disconnected", node: tracker.name, code });
    if (!isShuttingDown) scheduleReconnect(tracker);
  });

  ws.on("error", (err: Error) => {
    tracker.connected = false;
    log(`${tracker.name} WS error: ${err.message}`);
    emit({ event: "ws_error", node: tracker.name, error: err.message });
  });

  ws.on("pong", () => {
    tracker.lastEventTime = Date.now();
  });
}

function scheduleReconnect(tracker: NodeTracker) {
  if (isShuttingDown) return;
  if (tracker.reconnectTimer) return;

  tracker.reconnectAttempts++;
  const delay = Math.min(
    RECONNECT_BASE_MS * Math.pow(2, tracker.reconnectAttempts - 1),
    RECONNECT_MAX_MS
  );
  log(`${tracker.name} reconnecting in ${(delay / 1000).toFixed(1)}s (attempt ${tracker.reconnectAttempts})`);
  tracker.reconnectTimer = setTimeout(() => {
    tracker.reconnectTimer = null;
    connectNode(tracker);
  }, delay);
}

function handleNodeEvent(tracker: NodeTracker, msg: Record<string, unknown>) {
  const type = msg.type as string;
  const d = (msg.data ?? {}) as Record<string, unknown>;

  switch (type) {
    case "CheckpointCreated":
      handleCheckpoint(tracker, d);
      break;
    case "NewTransaction":
      tracker.txCount++;
      emit({
        event: "tx",
        node: tracker.name,
        hash: (d.hash as string)?.slice(0, 16),
        from: (d.from as string)?.slice(0, 12),
        to: (d.to as string)?.slice(0, 12),
        amount: d.amount,
        kind: d.kind,
      });
      break;
    case "FastPathConfirmed":
      tracker.fastPathCount++;
      emit({
        event: "fast_path_confirmed",
        node: tracker.name,
        hash: (d.hash as string)?.slice(0, 16),
        from: (d.from as string)?.slice(0, 12),
        to: (d.to as string)?.slice(0, 12),
        amount: d.amount,
        totalStake: d.total_stake,
        threshold: d.threshold,
      });
      break;
    case "FastPathExecuted":
      tracker.fastPathExecutedCount++;
      emit({
        event: "fast_path_executed",
        node: tracker.name,
        hash: (d.hash as string)?.slice(0, 16),
        from: (d.from as string)?.slice(0, 12),
        to: (d.to as string)?.slice(0, 12),
        amount: d.amount,
      });
      break;
    case "AccountUpdated":
      emit({
        event: "account_updated",
        node: tracker.name,
        address: (d.address as string)?.slice(0, 12),
        balance: d.balance,
        nonce: d.nonce,
      });
      break;
    case "PartitionSuspected":
    case "PartitionConfirmed":
    case "PartitionHealed":
    case "MergeStarted":
    case "MergeCompleted":
    case "MergeProgress":
    case "TransactionRolledBack":
    case "PenaltyAssessed":
      logAlert(`${tracker.name}: ${type} — ${JSON.stringify(d)}`);
      emit({ event: type.toLowerCase(), node: tracker.name, ...d });
      break;
    case "pong":
      break;
    default:
      emit({ event: "unknown", node: tracker.name, raw: msg });
      break;
  }
}

function handleCheckpoint(
  tracker: NodeTracker,
  d: Record<string, unknown>
) {
  const height = d.height as number;
  const hash = d.hash as string;
  const txsFinalized = d.txs_finalized as number;
  const reward = d.reward as number;

  if (typeof height !== "number") return;

  tracker.lastCheckpointHeight = height;

  log(
    `${tracker.name} CHECKPOINT h=${height} txs=${txsFinalized} reward=${reward?.toFixed?.(4) ?? reward} hash=${hash?.slice(0, 16)}...`
  );

  emit({
    event: "checkpoint",
    node: tracker.name,
    height,
    hash: hash?.slice(0, 24),
    txsFinalized,
    reward,
  });

  fetchCheckpointMerkle(tracker, height);
}

async function fetchCheckpointMerkle(tracker: NodeTracker, height: number) {
  try {
    const res = await fetch(`${tracker.url}/api/checkpoints/${height}`, {
      signal: AbortSignal.timeout(5000),
    });
    if (!res.ok) return;
    const cp = (await res.json()) as Record<string, unknown>;
    const root =
      (cp.tx_merkle_root as string) ??
      (cp.txMerkleRoot as string) ??
      (cp.merkle_root as string) ??
      (cp.merkleRoot as string) ??
      "";

    if (!root) return;

    tracker.checkpointMerkleRoots.set(height, root);
    if (tracker.checkpointMerkleRoots.size > MAX_MERKLE_CACHE) {
      const oldest = Math.min(...tracker.checkpointMerkleRoots.keys());
      tracker.checkpointMerkleRoots.delete(oldest);
    }

    let record = checkpointIndex.get(height);
    if (!record) {
      record = {
        height,
        hash: (cp.hash as string) ?? "",
        txsFinalized: (cp.txs_finalized as number) ?? 0,
        reward: (cp.reward as number) ?? 0,
        merkleRoots: new Map(),
        firstSeen: Date.now(),
        seenBy: [],
        forkAlerted: false,
        consensusLogged: false,
        confirmScheduled: false,
      };
      checkpointIndex.set(height, record);

      if (checkpointIndex.size > MAX_CHECKPOINT_INDEX) {
        const oldestKey = Math.min(...checkpointIndex.keys());
        checkpointIndex.delete(oldestKey);
      }
    }

    record.merkleRoots.set(tracker.name, root);
    if (!record.seenBy.includes(tracker.name)) {
      record.seenBy.push(tracker.name);
    }

    emit({
      event: "checkpoint_merkle",
      node: tracker.name,
      height,
      merkleRoot: root.slice(0, 24),
      seenBy: record.seenBy,
    });

    // Only evaluate consensus once every node has reported a root for this
    // height. Alerting as soon as any 2 nodes disagree produced false forks:
    // reads race with reorg settling, and a third (agreeing) value arrives a
    // moment later. Requiring all N — plus a confirm re-fetch below — makes the
    // signal reflect real, persistent divergence.
    if (record.merkleRoots.size === trackers.length && !record.forkAlerted) {
      const uniqueRoots = [...new Set(record.merkleRoots.values())];
      if (uniqueRoots.length === 1) {
        if (!record.consensusLogged) {
          record.consensusLogged = true;
          log(
            `  h=${height} CONSENSUS OK — all ${trackers.length} nodes agree: ${uniqueRoots[0]?.slice(0, 16)}...`
          );
        }
      } else if (!record.confirmScheduled) {
        // Divergent on this snapshot — re-fetch all nodes once and re-check
        // before alerting, to filter out transient reorg-settle mismatches.
        record.confirmScheduled = true;
        setTimeout(() => {
          for (const t of trackers) void fetchCheckpointMerkle(t, height);
        }, FORK_CONFIRM_MS);
      } else {
        // Still divergent after the confirm re-fetch — this is a real fork.
        record.forkAlerted = true;
        logAlert(
          `FORK at height ${height}! Roots: ${[...record.merkleRoots.entries()]
            .map(([n, r]) => `${n}=${r.slice(0, 16)}`)
            .join(", ")}`
        );
        if (forkAlerts.length < MAX_FORK_ALERTS) {
          forkAlerts.push({
            height,
            ts: Date.now(),
            roots: uniqueRoots,
          });
        }
        emit({
          event: "fork_detected",
          height,
          roots: Object.fromEntries(record.merkleRoots),
        });
      }
    }
  } catch {}
}

async function pollHealth() {
  for (const tracker of trackers) {
    try {
      const res = await fetch(`${tracker.url}/api/sync/status`, {
        signal: AbortSignal.timeout(5000),
      });
      if (!res.ok) continue;
      const data = (await res.json()) as Record<string, unknown>;

      let peerCount = 0;
      try {
        const peerRes = await fetch(`${tracker.url}/api/peers`, {
          signal: AbortSignal.timeout(3000),
        });
        if (peerRes.ok) {
          const peers = (await peerRes.json()) as Record<string, unknown[]>;
          peerCount =
            ((peers.httpPeers ?? peers.http_peers) as unknown[] ?? []).length +
            ((peers.p2pPeers ?? peers.p2p_peers) as unknown[] ?? []).length;
        }
      } catch {}

      const snap: HealthSnapshot = {
        ts: Date.now(),
        checkpointHeight: (data.checkpointHeight ?? data.checkpoint_height) as number ?? 0,
        dagSize: (data.dagSize ?? data.dag_size) as number ?? 0,
        tipCount: (data.tipCount ?? data.tip_count) as number ?? 0,
        totalTransactions: (data.totalTransactions ?? data.total_transactions) as number ?? 0,
        validators: data.validators as number ?? 0,
        totalStake: (data.totalStake ?? data.total_stake) as number ?? 0,
        faucetBalance: (data.faucetBalance ?? data.faucet_balance) as number ?? 0,
        peerCount,
      };

      tracker.healthSnapshots.push(snap);
      if (tracker.healthSnapshots.length > 1000) {
        tracker.healthSnapshots = tracker.healthSnapshots.slice(-500);
      }

      emit({
        event: "health",
        node: tracker.name,
        ...snap,
      });
    } catch {}
  }

  detectStalls();
  printLiveStatus();
}

function detectStalls() {
  const now = Date.now();
  for (const tracker of trackers) {
    if (!tracker.connected) continue;
    const silentMs = now - tracker.lastEventTime;
    if (silentMs > STALL_THRESHOLD_MS) {
      logAlert(
        `${tracker.name} stalled — no events for ${(silentMs / 1000).toFixed(1)}s`
      );
      emit({
        event: "stall_detected",
        node: tracker.name,
        silentMs,
      });

      if (tracker.ws) {
        try {
          tracker.ws.ping();
        } catch {}
      }
    }
  }

  const heights = trackers
    .filter((t) => t.connected)
    .map((t) => t.lastCheckpointHeight);
  if (heights.length >= 2) {
    const maxH = Math.max(...heights);
    const minH = Math.min(...heights);
    if (maxH - minH > 5) {
      logAlert(
        `Checkpoint height drift: ${trackers
          .map((t) => `${t.name}=h${t.lastCheckpointHeight}`)
          .join(", ")} (gap=${maxH - minH})`
      );
      emit({
        event: "height_drift",
        heights: Object.fromEntries(
          trackers.map((t) => [t.name, t.lastCheckpointHeight])
        ),
        gap: maxH - minH,
      });
    }
  }
}

function printLiveStatus() {
  const uptimeSec = ((Date.now() - globalStartTime) / 1000).toFixed(0);
  const lines: string[] = [];
  lines.push(`--- Status (uptime ${uptimeSec}s) ---`);
  for (const t of trackers) {
    const conn = t.connected ? "\x1b[32mON\x1b[0m" : "\x1b[31mOFF\x1b[0m";
    const lastSnap = t.healthSnapshots[t.healthSnapshots.length - 1];
    const dag = lastSnap?.dagSize ?? "?";
    const tips = lastSnap?.tipCount ?? "?";
    const peers = lastSnap?.peerCount ?? "?";
    const totalTx = lastSnap?.totalTransactions ?? "?";
    lines.push(
      `  ${t.name} [${conn}] h=${t.lastCheckpointHeight} dag=${dag} tips=${tips} peers=${peers} totalTx=${totalTx} wsTx=${t.txCount} fp=${t.fastPathCount}/${t.fastPathExecutedCount}`
    );
  }
  if (forkAlerts.length > 0) {
    lines.push(
      `  \x1b[31mFORKS: ${forkAlerts.length} detected\x1b[0m`
    );
  }
  lines.push(`  Log: ${JSONL_PATH}`);
  console.log(lines.join("\n"));
}

function shutdown() {
  if (isShuttingDown) return;
  isShuttingDown = true;

  log("Shutting down...");
  if (healthPollInterval) clearInterval(healthPollInterval);

  for (const t of trackers) {
    if (t.reconnectTimer) {
      clearTimeout(t.reconnectTimer);
      t.reconnectTimer = null;
    }
    if (t.ws) {
      t.ws.removeAllListeners();
      try {
        t.ws.close(1000);
      } catch {}
    }
  }

  const finalReport: string[] = [];
  finalReport.push("\n========== FINAL REPORT ==========");
  finalReport.push(`Session: ${sessionId}`);
  finalReport.push(
    `Duration: ${((Date.now() - globalStartTime) / 1000).toFixed(1)}s`
  );
  finalReport.push(`Checkpoints indexed: ${checkpointIndex.size}`);
  finalReport.push(`Forks detected: ${forkAlerts.length}`);

  for (const t of trackers) {
    finalReport.push(
      `  ${t.name}: h=${t.lastCheckpointHeight} txWs=${t.txCount} fp=${t.fastPathCount}/${t.fastPathExecutedCount}`
    );
  }

  if (forkAlerts.length > 0) {
    finalReport.push("Fork details:");
    for (const f of forkAlerts) {
      finalReport.push(
        `  h=${f.height} at ${new Date(f.ts).toISOString()} roots=[${f.roots
          .map((r) => r.slice(0, 16))
          .join(", ")}]`
      );
    }
  }

  const maxH = Math.max(
    ...trackers.map((t) => t.lastCheckpointHeight),
    0
  );
  let consensusStreak = 0;
  for (let h = maxH; h > 0; h--) {
    const rec = checkpointIndex.get(h);
    if (!rec || rec.merkleRoots.size < 2) break;
    const unique = new Set(rec.merkleRoots.values());
    if (unique.size === 1) {
      consensusStreak++;
    } else {
      break;
    }
  }
  finalReport.push(
    `Consecutive consensus checkpoints (from tip): ${consensusStreak}`
  );

  const report = finalReport.join("\n");
  console.log(report);
  summaryStream.write(report + "\n");

  emit({
    event: "session_end",
    checkpointsIndexed: checkpointIndex.size,
    forksDetected: forkAlerts.length,
    consensusStreak,
    nodes: trackers.map((t) => ({
      name: t.name,
      finalHeight: t.lastCheckpointHeight,
      txCount: t.txCount,
      fastPathCount: t.fastPathCount,
      fastPathExecutedCount: t.fastPathExecutedCount,
    })),
  });

  jsonlStream.end(() => {
    summaryStream.end(() => {
      process.exit(0);
    });
  });
}

process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);

log(`Rinku Testnet Monitor v1.0`);
log(`Session: ${sessionId}`);
log(`Nodes: ${nodes.join(", ")}`);
log(`JSONL log: ${JSONL_PATH}`);
log(`Summary: ${SUMMARY_PATH}`);
log("");

globalStartTime = Date.now();

for (let i = 0; i < nodes.length; i++) {
  const tracker = initTracker(nodes[i], i);
  trackers.push(tracker);
  connectNode(tracker);
}

healthPollInterval = setInterval(pollHealth, HEALTH_POLL_MS);
setTimeout(pollHealth, 2000);
