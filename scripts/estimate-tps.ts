#!/usr/bin/env tsx

const NODE_URL = process.env.NODE_URL || "http://localhost:3001";
const WINDOW_SECS = parseInt(process.env.WINDOW_SECS || "120", 10);
const SAMPLE_MS = parseInt(process.env.SAMPLE_MS || "1000", 10);
const ASSUMED_PROPAGATION_MS = process.env.ASSUMED_PROPAGATION_MS
  ? parseInt(process.env.ASSUMED_PROPAGATION_MS, 10)
  : undefined;

type StatsResponse = {
  dag_nodes: number;
  tips: number;
  accounts: number;
  checkpoint_height: number;
  gas_price: number;
  total_supply: number;
  validators: number;
  total_stake: number;
};

type CheckpointInfo = {
  height: number;
  merkle_root: string;
  tx_count: number;
  timestamp: number;
  validators: number;
};

type CheckpointsResponse = {
  checkpoints: CheckpointInfo[];
  total: number;
};

type Sample = {
  at: number;
  dagNodes: number;
  tips: number;
  checkpointHeight: number;
};

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`HTTP ${res.status}: ${text}`);
  }
  return res.json() as Promise<T>;
}

function avg(values: number[]): number {
  if (values.length === 0) return 0;
  return values.reduce((a, b) => a + b, 0) / values.length;
}

function fmt(num: number, digits: number = 2): string {
  return Number.isFinite(num) ? num.toFixed(digits) : "n/a";
}

async function main(): Promise<void> {
  const start = Date.now();
  const endAt = start + WINDOW_SECS * 1000;

  console.log("=".repeat(60));
  console.log("RINKU TPS ESTIMATOR");
  console.log("=".repeat(60));
  console.log(`Node: ${NODE_URL}`);
  console.log(`Window: ${WINDOW_SECS}s, Sample: ${SAMPLE_MS}ms`);
  if (ASSUMED_PROPAGATION_MS !== undefined) {
    console.log(`Assumed propagation: ${ASSUMED_PROPAGATION_MS}ms`);
  }
  console.log("=".repeat(60));

  const samples: Sample[] = [];
  const checkpoints: CheckpointInfo[] = [];
  let lastCheckpointHeight = -1;

  while (Date.now() < endAt) {
    const [stats, latest] = await Promise.all([
      fetchJson<StatsResponse>(`${NODE_URL}/api/stats`),
      fetchJson<CheckpointInfo>(`${NODE_URL}/api/checkpoints/latest`),
    ]);

    samples.push({
      at: Date.now(),
      dagNodes: stats.dag_nodes,
      tips: stats.tips,
      checkpointHeight: stats.checkpoint_height,
    });

    if (latest.height > lastCheckpointHeight) {
      checkpoints.push(latest);
      lastCheckpointHeight = latest.height;
    }

    await new Promise((r) => setTimeout(r, SAMPLE_MS));
  }

  if (samples.length < 2) {
    console.error("Not enough samples collected to estimate TPS.");
    process.exit(1);
  }

  const first = samples[0];
  const last = samples[samples.length - 1];
  const elapsedSec = (last.at - first.at) / 1000;

  const ingestTps = (last.dagNodes - first.dagNodes) / elapsedSec;
  const avgTips = avg(samples.map((s) => s.tips));
  const avgCheckpointHeight = avg(samples.map((s) => s.checkpointHeight));

  let finalizedTps: number | null = null;
  let checkpointRate: number | null = null;
  if (checkpoints.length >= 2) {
    const sorted = checkpoints
      .slice()
      .sort((a, b) => a.height - b.height);
    const firstCp = sorted[0];
    const lastCp = sorted[sorted.length - 1];
    const cpElapsedSec = lastCp.timestamp - firstCp.timestamp;
    if (cpElapsedSec > 0) {
      const txFinalized = sorted
        .slice(1)
        .reduce((sum, cp) => sum + cp.tx_count, 0);
      finalizedTps = txFinalized / cpElapsedSec;
      checkpointRate = (lastCp.height - firstCp.height) / cpElapsedSec;
    }
  }

  let latencyBoundTps: number | null = null;
  if (ASSUMED_PROPAGATION_MS !== undefined && ASSUMED_PROPAGATION_MS > 0) {
    latencyBoundTps = avgTips / (ASSUMED_PROPAGATION_MS / 1000);
  }

  const estimateCandidates = [
    ingestTps,
    finalizedTps ?? Infinity,
    latencyBoundTps ?? Infinity,
  ].filter((v) => Number.isFinite(v));
  const estimatedMaxTps = Math.min(...estimateCandidates);

  console.log("\nRESULTS");
  console.log("-".repeat(60));
  console.log(`Observed ingest TPS (DAG growth): ${fmt(ingestTps)}`);
  console.log(`Avg tip count: ${fmt(avgTips, 1)}`);
  console.log(`Avg checkpoint height: ${fmt(avgCheckpointHeight, 2)}`);
  if (finalizedTps !== null) {
    console.log(
      `Finalized TPS (checkpoint tx_count): ${fmt(finalizedTps)}`
    );
  } else {
    console.log("Finalized TPS (checkpoint tx_count): n/a (not enough checkpoints)");
  }
  if (checkpointRate !== null) {
    console.log(`Checkpoint rate: ${fmt(checkpointRate, 3)} / sec`);
  }
  if (latencyBoundTps !== null) {
    console.log(
      `Latency-bound TPS (tips / propagation): ${fmt(latencyBoundTps)}`
    );
  }
  console.log("-".repeat(60));
  console.log(`Estimated max TPS (min of signals): ${fmt(estimatedMaxTps)}`);

  console.log("\nASSUMPTIONS");
  console.log("-".repeat(60));
  console.log(
    "- DAG ingest TPS uses dag_nodes delta; assumes each node ~= 1 tx."
  );
  console.log(
    "- Finalized TPS uses checkpoint tx_count; tx_count is checkpoint tip_count."
  );
  console.log(
    "- Latency-bound TPS uses avg tips / assumed propagation if provided."
  );
  console.log("-".repeat(60));

  const recent = await fetchJson<CheckpointsResponse>(
    `${NODE_URL}/api/checkpoints`
  );
  const recentHeights = recent.checkpoints
    .slice(0, 5)
    .map((c) => c.height)
    .join(", ");
  console.log(`Recent checkpoints (latest 5): ${recentHeights}`);
  console.log("=".repeat(60));
}

main().catch((err) => {
  console.error("TPS estimator failed:", err);
  process.exit(1);
});
