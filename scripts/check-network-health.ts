#!/usr/bin/env npx tsx
/**
 * Network health gate for CI / cron.
 *
 * Checks: node health, protocol version, checkpoint consensus, P2P peers,
 * optional height-stall detection (via --prev-heights-file), optional
 * "expect load" age threshold.
 *
 * Usage:
 *   npx tsx scripts/check-network-health.ts \
 *     https://rinku-genesis.fly.dev \
 *     https://rinku-validator-1.fly.dev \
 *     https://rinku-validator-2.fly.dev
 *
 * Env:
 *   MAX_HEIGHT_SPREAD=2
 *   MIN_PEER_COUNT=1
 *   MAX_CHECKPOINT_AGE_MS=0          # 0 = disabled; e.g. 1800000 = 30m when load expected
 *   EXPECT_LOAD=false               # if true, MAX_CHECKPOINT_AGE_MS defaults to 30m
 *   PREV_HEIGHTS_FILE=              # JSON map url→height from prior run; stall if unchanged
 *   STALL_FAIL=true                 # fail when prev heights match and EXPECT_LOAD/age triggers
 */
const nodes = process.argv.slice(2).filter((a) => !a.startsWith("--"));
if (nodes.length === 0) {
  console.error(
    "Usage: npx tsx scripts/check-network-health.ts NODE_URL [NODE_URL...]"
  );
  process.exit(1);
}

const MAX_HEIGHT_SPREAD = parseInt(process.env.MAX_HEIGHT_SPREAD || "2", 10);
const MIN_PEER_COUNT = parseInt(process.env.MIN_PEER_COUNT || "1", 10);
const EXPECT_LOAD =
  process.env.EXPECT_LOAD === "true" || process.env.EXPECT_LOAD === "1";
const MAX_CHECKPOINT_AGE_MS = parseInt(
  process.env.MAX_CHECKPOINT_AGE_MS ||
    (EXPECT_LOAD ? String(30 * 60 * 1000) : "0"),
  10
);
const PREV_HEIGHTS_FILE = process.env.PREV_HEIGHTS_FILE || "";
const STALL_FAIL =
  process.env.STALL_FAIL !== "false" && process.env.STALL_FAIL !== "0";

interface NodeSnapshot {
  url: string;
  ok: boolean;
  errors: string[];
  protocolVersion?: string;
  height?: number;
  peerCount?: number;
  lastCheckpointAgeMs?: number;
  tps?: number;
  totalTransactions?: number;
}

async function fetchJson(url: string, path: string, timeoutMs = 10000): Promise<any> {
  const ctrl = new AbortController();
  const t = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    const res = await fetch(`${url}${path}`, { signal: ctrl.signal });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return await res.json();
  } finally {
    clearTimeout(t);
  }
}

async function snapshotNode(url: string): Promise<NodeSnapshot> {
  const snap: NodeSnapshot = { url, ok: true, errors: [] };
  try {
    const health = await fetchJson(url, "/health");
    if (health.status !== "ok") {
      snap.ok = false;
      snap.errors.push(`health status=${health.status}`);
    }
  } catch (e: any) {
    snap.ok = false;
    snap.errors.push(`health: ${e.message}`);
    return snap;
  }

  try {
    const ver = await fetchJson(url, "/api/version");
    snap.protocolVersion = ver.protocolVersion;
  } catch (e: any) {
    snap.ok = false;
    snap.errors.push(`version: ${e.message}`);
  }

  try {
    const stats = await fetchJson(url, "/api/network/stats");
    snap.height = Number(stats.latestCheckpointHeight ?? stats.checkpointCount ?? 0);
    snap.tps = Number(stats.tps ?? 0);
    snap.totalTransactions = Number(stats.totalTransactionsProcessed ?? 0);
  } catch (e: any) {
    snap.ok = false;
    snap.errors.push(`network/stats: ${e.message}`);
  }

  try {
    const peers = await fetchJson(url, "/api/peers");
    snap.peerCount = Number(
      peers.peerCount ?? (peers.peers?.length ?? 0)
    );
  } catch (e: any) {
    snap.ok = false;
    snap.errors.push(`peers: ${e.message}`);
  }

  try {
    const fin = await fetchJson(url, "/api/finality/metrics");
    snap.lastCheckpointAgeMs = Number(fin.lastCheckpointAge ?? NaN);
  } catch {
    // optional on older nodes
  }

  return snap;
}

function loadPrevHeights(path: string): Record<string, number> {
  if (!path) return {};
  try {
    const fs = require("fs") as typeof import("fs");
    if (!fs.existsSync(path)) return {};
    return JSON.parse(fs.readFileSync(path, "utf8"));
  } catch {
    return {};
  }
}

function saveHeights(path: string, heights: Record<string, number>) {
  if (!path) return;
  const fs = require("fs") as typeof import("fs");
  fs.writeFileSync(path, JSON.stringify(heights, null, 2) + "\n");
}

async function main() {
  console.log("=== Rinku network health ===");
  console.log(`Nodes: ${nodes.length}`);
  console.log(
    `Thresholds: spread≤${MAX_HEIGHT_SPREAD} peers≥${MIN_PEER_COUNT} expectLoad=${EXPECT_LOAD} maxAgeMs=${MAX_CHECKPOINT_AGE_MS || "off"}`
  );

  const snaps = await Promise.all(nodes.map(snapshotNode));
  let failed = false;

  for (const s of snaps) {
    const peerOk =
      s.peerCount === undefined ? false : s.peerCount >= MIN_PEER_COUNT;
    console.log(
      `\n${s.url}\n  health=${s.ok ? "ok" : "FAIL"} height=${s.height ?? "?"} peers=${s.peerCount ?? "?"} ver=${s.protocolVersion ?? "?"} ageMs=${s.lastCheckpointAgeMs ?? "?"} tps=${s.tps ?? "?"}`
    );
    if (!s.ok) {
      failed = true;
      for (const e of s.errors) console.log(`  ✗ ${e}`);
    }
    if (s.peerCount !== undefined && !peerOk) {
      failed = true;
      console.log(
        `  ✗ P2P peer count ${s.peerCount} < min ${MIN_PEER_COUNT}`
      );
    } else if (peerOk) {
      console.log(`  ✓ P2P peers ${s.peerCount}`);
    }
  }

  const heights = snaps
    .map((s) => s.height)
    .filter((h): h is number => typeof h === "number");
  if (heights.length === snaps.length && heights.length > 1) {
    const min = Math.min(...heights);
    const max = Math.max(...heights);
    const spread = max - min;
    if (spread > MAX_HEIGHT_SPREAD) {
      failed = true;
      console.log(
        `\n✗ Checkpoint height spread ${spread} > ${MAX_HEIGHT_SPREAD} (${heights.join(", ")})`
      );
    } else {
      console.log(`\n✓ Checkpoint consensus spread=${spread} (${heights.join(", ")})`);
    }
  }

  const versions = snaps
    .map((s) => s.protocolVersion)
    .filter((v): v is string => !!v);
  if (versions.length > 1 && new Set(versions).size > 1) {
    failed = true;
    console.log(`\n✗ Protocol version mismatch: ${versions.join(", ")}`);
  } else if (versions.length > 0) {
    console.log(`✓ Protocol version ${versions[0]}`);
  }

  // Age-based stall (only when configured / expect load)
  if (MAX_CHECKPOINT_AGE_MS > 0) {
    for (const s of snaps) {
      if (
        typeof s.lastCheckpointAgeMs === "number" &&
        !Number.isNaN(s.lastCheckpointAgeMs) &&
        s.lastCheckpointAgeMs > MAX_CHECKPOINT_AGE_MS
      ) {
        const msg = `checkpoint age ${s.lastCheckpointAgeMs}ms > ${MAX_CHECKPOINT_AGE_MS}ms on ${s.url}`;
        if (EXPECT_LOAD && STALL_FAIL) {
          failed = true;
          console.log(`\n✗ Stall: ${msg}`);
        } else {
          console.log(`\n⚠ Idle/stall signal: ${msg}`);
        }
      }
    }
  }

  // Cross-run stall via previous heights file
  const prev = loadPrevHeights(PREV_HEIGHTS_FILE);
  const current: Record<string, number> = {};
  for (const s of snaps) {
    if (typeof s.height === "number") current[s.url] = s.height;
  }
  if (PREV_HEIGHTS_FILE && Object.keys(prev).length > 0) {
    const stalled = snaps.filter(
      (s) =>
        typeof s.height === "number" &&
        prev[s.url] !== undefined &&
        prev[s.url] === s.height
    );
    if (stalled.length === snaps.length) {
      const msg = `all ${stalled.length} nodes still at prior heights (${Object.values(current).join(", ")})`;
      if ((EXPECT_LOAD || MAX_CHECKPOINT_AGE_MS > 0) && STALL_FAIL) {
        failed = true;
        console.log(`\n✗ Height stall across intervals: ${msg}`);
      } else {
        console.log(`\n⚠ Height unchanged since last run: ${msg}`);
      }
    } else {
      console.log("✓ Height advanced since last run on at least one node");
    }
  }
  if (PREV_HEIGHTS_FILE) {
    saveHeights(PREV_HEIGHTS_FILE, current);
    console.log(`Wrote heights → ${PREV_HEIGHTS_FILE}`);
  }

  if (failed) {
    console.log("\nNETWORK HEALTH FAILED");
    process.exit(1);
  }
  console.log("\nNETWORK HEALTH OK");
  process.exit(0);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
