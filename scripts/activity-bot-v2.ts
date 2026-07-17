#!/usr/bin/env npx tsx
import { Wallet } from "@rinku/wallet";
import WebSocket from "ws";

const NODE_URLS = (
  process.env.RINKU_NODE_URLS ||
  process.env.RINKU_NODE_URL ||
  "http://localhost:3001"
)
  .split(",")
  .map((url) => url.trim())
  .filter(Boolean);

const PRIMARY_NODE_URL = NODE_URLS[0] || "http://localhost:3001";

const MODE = (process.argv.find((a) => a.startsWith("--mode="))?.split("=")[1] ||
  process.env.BOT_MODE ||
  "realistic") as "realistic" | "stress";

const TARGET_TPS = parseInt(
  process.argv.find((a) => a.startsWith("--tps="))?.split("=")[1] ||
    process.env.TARGET_TPS ||
    "5"
);

const ACCOUNT_COUNT = parseInt(
  process.argv.find((a) => a.startsWith("--accounts="))?.split("=")[1] ||
    process.env.ACCOUNT_COUNT ||
    (MODE === "stress" ? "50" : "10")
);

const DURATION_S = parseInt(
  process.argv.find((a) => a.startsWith("--duration="))?.split("=")[1] ||
    process.env.DURATION_S ||
    "300"
);

/** duration=0 (or negative) runs until SIGINT/SIGTERM — for always-on Fly bots */
const RUN_FOREVER = !Number.isFinite(DURATION_S) || DURATION_S <= 0;

const MAX_CONCURRENT_REQUESTS = (() => {
  const raw = parseInt(
    process.argv.find((a) => a.startsWith("--concurrency="))?.split("=")[1] ||
      process.env.MAX_CONCURRENT_REQUESTS ||
      "50"
  );
  return Number.isFinite(raw) && raw >= 1 ? Math.floor(raw) : 50;
})();

const FAUCET_AMOUNT = 100;
const FETCH_TIMEOUT_MS = 10000;
const WS_CONFIRM_TIMEOUT_MS = 5000;

class Semaphore {
  private current = 0;
  private queue: (() => void)[] = [];

  constructor(private max: number) {}

  async acquire(): Promise<void> {
    if (this.current < this.max) {
      this.current++;
      return;
    }
    return new Promise<void>((resolve) => {
      this.queue.push(() => {
        this.current++;
        resolve();
      });
    });
  }

  release(): void {
    if (this.current <= 0) return;
    this.current--;
    if (this.queue.length > 0) {
      const next = this.queue.shift()!;
      next();
    }
  }

  get inflight(): number {
    return this.current;
  }
}

const httpSemaphore = new Semaphore(MAX_CONCURRENT_REQUESTS);

interface AccountSlot {
  wallet: Wallet;
  fingerprint: string;
  nonce: number;
  balance: number;
  nodeUrl: string;
  busy: boolean;
  pendingHash: string | null;
  pendingResolve: ((confirmed: boolean) => void) | null;
  pendingTimer: ReturnType<typeof setTimeout> | null;
  txCount: number;
}

interface Stats {
  txSent: number;
  txConfirmed: number;
  txFailed: number;
  txTimedOut: number;
  latencies: number[];
  startTime: number;
  errors: number;
  lastReportTime: number;
  lastReportTxSent: number;
}

const accounts: AccountSlot[] = [];
const stats: Stats = {
  txSent: 0,
  txConfirmed: 0,
  txFailed: 0,
  txTimedOut: 0,
  latencies: [],
  startTime: 0,
  errors: 0,
  lastReportTime: 0,
  lastReportTxSent: 0,
};

let wsConnections: WebSocket[] = [];
const pendingConfirmations = new Map<
  string,
  { sentAt: number; slot: AccountSlot }
>();
let running = true;
let currentGasPrice = 0.001;

function log(msg: string) {
  const ts = new Date().toISOString().slice(11, 23);
  console.log(`[${ts}] ${msg}`);
}

async function fetchWithTimeout(
  url: string,
  options: RequestInit = {},
  timeoutMs: number = FETCH_TIMEOUT_MS
): Promise<Response> {
  await httpSemaphore.acquire();
  try {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), timeoutMs);
    return await fetch(url, { ...options, signal: controller.signal }).finally(() =>
      clearTimeout(timeout)
    );
  } finally {
    httpSemaphore.release();
  }
}

function assignNodeUrl(index: number): string {
  return NODE_URLS[index % NODE_URLS.length];
}

async function fetchGasPrice(): Promise<number> {
  try {
    const res = await fetchWithTimeout(
      `${PRIMARY_NODE_URL}/api/gas/price`,
      {},
      3000
    );
    if (res.ok) {
      const data = (await res.json()) as { current: number };
      if (typeof data.current === "number") {
        currentGasPrice = data.current * 1.15;
      }
    }
  } catch {}
  return currentGasPrice;
}

async function faucetRequest(fingerprint: string): Promise<boolean> {
  try {
    const res = await fetchWithTimeout(`${PRIMARY_NODE_URL}/api/request`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ address: fingerprint }),
    });
    return res.ok;
  } catch {
    return false;
  }
}

async function queryNonce(
  fingerprint: string,
  nodeUrl: string
): Promise<number> {
  try {
    const res = await fetchWithTimeout(
      `${nodeUrl}/api/account/${fingerprint}`,
      {},
      3000
    );
    if (res.ok) {
      const data = (await res.json()) as { effective_nonce?: number; nonce?: number };
      return data.effective_nonce ?? data.nonce ?? 0;
    }
  } catch {}
  return 0;
}

async function queryHighestNonce(fingerprint: string): Promise<number> {
  const urls = NODE_URLS.length > 0 ? NODE_URLS : [PRIMARY_NODE_URL];
  const results = await Promise.allSettled(
    urls.map(async (url) => {
      const res = await fetchWithTimeout(
        `${url}/api/account/${fingerprint}`,
        {},
        3000
      );
      if (res.ok) {
        const data = (await res.json()) as { effective_nonce?: number; nonce?: number };
        return data.effective_nonce ?? data.nonce ?? 0;
      }
      return 0;
    })
  );
  let maxNonce = 0;
  for (const r of results) {
    if (r.status === "fulfilled" && r.value > maxNonce) {
      maxNonce = r.value;
    }
  }
  return maxNonce;
}

async function queryBalance(
  fingerprint: string,
  nodeUrl: string
): Promise<number> {
  try {
    const res = await fetchWithTimeout(
      `${nodeUrl}/api/account/${fingerprint}`,
      {},
      3000
    );
    if (res.ok) {
      const data = (await res.json()) as { balance?: number };
      return data.balance ?? 0;
    }
  } catch {}
  return 0;
}

const tipCache = new Map<string, { tips: string[]; at: number }>();
const tipInflight = new Map<string, Promise<string[]>>();
const TIP_CACHE_TTL_MS = 500;

async function getTipParents(nodeUrl: string): Promise<string[]> {
  const cached = tipCache.get(nodeUrl);
  if (cached && Date.now() - cached.at < TIP_CACHE_TTL_MS) {
    return cached.tips;
  }
  const existing = tipInflight.get(nodeUrl);
  if (existing) return existing;

  const p = (async () => {
    try {
      const res = await fetchWithTimeout(`${nodeUrl}/api/tips`, {}, 3000);
      if (res.ok) {
        const data = (await res.json()) as { tips: string[] };
        const tips = (data.tips || []).slice(0, 2).map((h) => `rinku://tx/h/${h}`);
        tipCache.set(nodeUrl, { tips, at: Date.now() });
        return tips;
      }
    } catch {}
    if (cached) return cached.tips;
    return [];
  })();

  tipInflight.set(nodeUrl, p);
  try {
    return await p;
  } finally {
    tipInflight.delete(nodeUrl);
  }
}

function connectWS(nodeUrl: string): WebSocket {
  const wsUrl = nodeUrl.replace(/^http/, "ws") + "/api/ws";
  const ws = new WebSocket(wsUrl);

  ws.on("open", () => {
    log(`WS connected: ${nodeUrl}`);
  });

  ws.on("message", (raw) => {
    try {
      const msg = JSON.parse(raw.toString());
      if (!msg.type || !msg.data) return;

      if (msg.type === "FastPathExecuted") {
        const hash = msg.data.hash as string;
        if (!hash) return;

        const fullHash = hash;
        let pending = pendingConfirmations.get(fullHash);
        if (!pending) {
          const shortHash = hash.slice(0, 16);
          pending = pendingConfirmations.get(shortHash);
          if (!pending) return;
          pendingConfirmations.delete(shortHash);
          pendingConfirmations.set(fullHash, pending);
        }
        if (!pending) return;

        const latencyMs = Date.now() - pending.sentAt;
        stats.txConfirmed++;
        stats.latencies.push(latencyMs);

        if (pending.slot.pendingTimer) {
          clearTimeout(pending.slot.pendingTimer);
          pending.slot.pendingTimer = null;
        }

        pending.slot.busy = false;
        pending.slot.pendingHash = null;
        pendingConfirmations.delete(fullHash);

        if (pending.slot.pendingResolve) {
          pending.slot.pendingResolve(true);
          pending.slot.pendingResolve = null;
        }
      }
    } catch {}
  });

  ws.on("close", () => {
    if (running) {
      log(`WS disconnected: ${nodeUrl}, reconnecting in 2s...`);
      setTimeout(() => {
        if (running) {
          const newWs = connectWS(nodeUrl);
          const idx = wsConnections.indexOf(ws);
          if (idx >= 0) wsConnections[idx] = newWs;
          else wsConnections.push(newWs);
        }
      }, 2000);
    }
  });

  ws.on("error", () => {});

  return ws;
}

async function createAndFundAccount(index: number): Promise<AccountSlot> {
  const nodeUrl = assignNodeUrl(index);
  const wallet = new Wallet(nodeUrl);
  const fingerprint = await wallet.create();

  const success = await faucetRequest(fingerprint);
  if (!success) {
    throw new Error(`Faucet failed for account ${index}`);
  }

  for (let attempt = 0; attempt < 5; attempt++) {
    await new Promise((r) => setTimeout(r, 600));
    await wallet.refresh();
    if (wallet.getNonce() > 0 || (await wallet.getBalance()) > 0) break;
  }

  const maxNonce = await queryHighestNonce(fingerprint);
  const nonce = Math.max(wallet.getNonce(), maxNonce);
  const balance = await wallet.getBalance();

  return {
    wallet,
    fingerprint,
    nonce,
    balance: balance || FAUCET_AMOUNT,
    nodeUrl,
    busy: false,
    pendingHash: null,
    pendingResolve: null,
    pendingTimer: null,
    txCount: 0,
  };
}

async function wallet_resync(slot: AccountSlot): Promise<void> {
  await slot.wallet.refresh();
  const highNonce = await queryHighestNonce(slot.fingerprint);
  slot.nonce = Math.max(slot.wallet.getNonce(), highNonce);
  slot.balance = await slot.wallet.getBalance();
}

async function postTxWithRetry(
  url: string,
  body: string,
  senderFingerprint: string,
  expectedNonce: number
): Promise<Response> {
  try {
    return await fetchWithTimeout(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body,
    });
  } catch (firstErr: any) {
    const msg = firstErr?.message || "";
    const isTransient =
      msg.includes("fetch failed") || msg.includes("aborted");
    if (!isTransient) throw firstErr;

    await new Promise((r) => setTimeout(r, 50 + Math.random() * 100));

    try {
      const confirmedNonce = await queryHighestNonce(senderFingerprint);
      if (confirmedNonce > expectedNonce) {
        throw Object.assign(new Error("TX likely succeeded (nonce advanced)"), {
          likelySucceeded: true,
          confirmedNonce,
        });
      }
    } catch (nonceErr: any) {
      if (nonceErr?.likelySucceeded) throw nonceErr;
    }

    return await fetchWithTimeout(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body,
    });
  }
}

async function sendTransaction(
  sender: AccountSlot,
  recipient: AccountSlot,
  amount: number
): Promise<boolean> {
  try {
    const freshNonce = await queryNonce(sender.fingerprint, sender.nodeUrl);
    if (freshNonce > 0 && freshNonce !== sender.nonce) {
      sender.nonce = freshNonce;
    }

    const tipUrls = await getTipParents(sender.nodeUrl);
    const fee = currentGasPrice;

    const signedTx = await sender.wallet.createSignedTransactionWithOptions({
      to: recipient.fingerprint,
      amount,
      fee,
      nonce: sender.nonce,
      tipUrls,
      skipRefresh: true,
    });

    const publicKey = await sender.wallet.getPublicKey();
    const txBody = JSON.stringify({ tx: signedTx, publicKey: Array.from(publicKey) });
    const res = await postTxWithRetry(
      `${sender.nodeUrl}/api/tx`,
      txBody,
      sender.fingerprint,
      sender.nonce
    );

    if (res.ok) {
      const data = (await res.json()) as { hash?: string };
      const hash = data.hash || "";

      sender.nonce++;
      sender.balance -= amount + fee;
      recipient.balance += amount;
      sender.txCount++;
      stats.txSent++;

      sender.busy = true;
      sender.pendingHash = hash;

      const sentAt = Date.now();
      pendingConfirmations.set(hash, { sentAt, slot: sender });

      sender.pendingTimer = setTimeout(async () => {
        if (!pendingConfirmations.has(hash)) return;
        pendingConfirmations.delete(hash);
        sender.pendingHash = null;
        stats.txTimedOut++;
        try {
          await new Promise((r) => setTimeout(r, 500));
          let confirmedNonce = await queryNonce(sender.fingerprint, sender.nodeUrl);
          if (confirmedNonce <= 0) {
            confirmedNonce = await queryHighestNonce(sender.fingerprint);
          }
          if (confirmedNonce > 0) {
            sender.nonce = confirmedNonce;
          }
        } catch {
        } finally {
          sender.busy = false;
          if (sender.pendingResolve) {
            sender.pendingResolve(false);
            sender.pendingResolve = null;
          }
        }
      }, WS_CONFIRM_TIMEOUT_MS);

      return true;
    } else {
      const errData = (await res.json().catch(() => ({}))) as {
        error?: string;
      };
      const errMsg = errData.error || "";

      if (errMsg.toLowerCase().includes("gas price too low")) {
        const minMatch = errMsg.match(/minimum is ([\d.]+)/);
        if (minMatch) {
          currentGasPrice = parseFloat(minMatch[1]) * 1.15;
        } else {
          fetchGasPrice();
        }
      } else if (errMsg.toLowerCase().includes("nonce")) {
        await new Promise((r) => setTimeout(r, 300));
        const expectedMatch = errMsg.match(/expected\s+(\d+)/i);
        if (expectedMatch) {
          sender.nonce = parseInt(expectedMatch[1], 10);
        } else {
          sender.nonce = await queryHighestNonce(sender.fingerprint);
        }
      } else if (errMsg.toLowerCase().includes("does not exist")) {
        await wallet_resync(sender);
      } else if (errMsg.toLowerCase().includes("insufficient balance")) {
        sender.balance = await queryBalance(sender.fingerprint, sender.nodeUrl);
      }
      log(`TX failed (${sender.fingerprint.slice(0, 12)}): ${errMsg.slice(0, 80)}`);
      stats.txFailed++;
      stats.errors++;
      return false;
    }
  } catch (err: any) {
    if (err?.likelySucceeded) {
      sender.nonce = err.confirmedNonce;
      sender.balance -= amount + currentGasPrice;
      recipient.balance += amount;
      sender.txCount++;
      stats.txSent++;
      stats.txConfirmed++;
      return true;
    }
    log(`TX error (${sender.fingerprint.slice(0, 12)}): ${(err?.message || "").slice(0, 80)}`);
    stats.txFailed++;
    stats.errors++;
    return false;
  }
}

function waitForConfirmation(slot: AccountSlot): Promise<boolean> {
  if (!slot.busy) return Promise.resolve(true);
  return new Promise((resolve) => {
    slot.pendingResolve = resolve;
  });
}

function pickRandomOther(slots: AccountSlot[], exclude: AccountSlot): AccountSlot {
  const others = slots.filter((s) => s !== exclude);
  return others[Math.floor(Math.random() * others.length)];
}

function percentile(arr: number[], p: number): number {
  if (arr.length === 0) return 0;
  const sorted = [...arr].sort((a, b) => a - b);
  const idx = Math.floor(sorted.length * p);
  return sorted[Math.min(idx, sorted.length - 1)];
}

function reportStats() {
  const now = Date.now();
  const elapsed = (now - stats.startTime) / 1000;
  const windowElapsed = (now - stats.lastReportTime) / 1000;
  const windowTx = stats.txSent - stats.lastReportTxSent;
  const windowTps = windowElapsed > 0 ? windowTx / windowElapsed : 0;
  const overallTps = elapsed > 0 ? stats.txSent / elapsed : 0;

  const recentLatencies = stats.latencies.slice(-200);
  const p50 = percentile(recentLatencies, 0.5);
  const p95 = percentile(recentLatencies, 0.95);
  const p99 = percentile(recentLatencies, 0.99);
  const max =
    recentLatencies.length > 0 ? Math.max(...recentLatencies) : 0;

  const busyCount = accounts.filter((a) => a.busy).length;
  const pendingCount = pendingConfirmations.size;

  log(
    `STATS | sent=${stats.txSent} confirmed=${stats.txConfirmed} failed=${stats.txFailed} timedOut=${stats.txTimedOut} ` +
      `| TPS: ${windowTps.toFixed(1)} (window) ${overallTps.toFixed(1)} (overall) ` +
      `| latency p50=${p50}ms p95=${p95}ms p99=${p99}ms max=${max}ms ` +
      `| busy=${busyCount}/${accounts.length} pending=${pendingCount} inflight=${httpSemaphore.inflight}/${MAX_CONCURRENT_REQUESTS} ` +
      `| gas=${currentGasPrice.toFixed(4)}`
  );

  stats.lastReportTime = now;
  stats.lastReportTxSent = stats.txSent;
}

async function refillLowBalances() {
  const lowAccounts = accounts.filter((a) => a.balance < 10 && !a.busy);
  for (const acc of lowAccounts.slice(0, 5)) {
    const success = await faucetRequest(acc.fingerprint);
    if (success) {
      acc.balance += FAUCET_AMOUNT;
      log(`Refilled ${acc.fingerprint.slice(0, 12)}...`);
    }
  }
}

async function runRealisticMode() {
  log(
    `REALISTIC MODE: ${ACCOUNT_COUNT} accounts, natural send-wait-send pattern, ${RUN_FOREVER ? "continuous" : `${DURATION_S}s`} duration`
  );

  async function accountLoop(slot: AccountSlot) {
    while (running) {
      const minDelay = 800 + Math.random() * 2200;
      await new Promise((r) => setTimeout(r, minDelay));

      if (!running) break;
      if (slot.busy) {
        await waitForConfirmation(slot);
        continue;
      }
      if (slot.balance < currentGasPrice + 2) {
        await new Promise((r) => setTimeout(r, 5000));
        continue;
      }

      const recipient = pickRandomOther(accounts, slot);
      const amount = Math.max(0.001, Number((Math.random() * 0.99).toFixed(3)));

      const sent = await sendTransaction(slot, recipient, amount);
      if (sent) {
        await waitForConfirmation(slot);
      }
    }
  }

  const loops = accounts.map((slot) => accountLoop(slot));
  await Promise.all(loops);
}

async function runStressMode() {
  log(
    `STRESS MODE: ${ACCOUNT_COUNT} accounts, target ${TARGET_TPS} TPS, ${RUN_FOREVER ? "continuous" : `${DURATION_S}s`} duration`
  );

  const txIntervalMs = 1000 / TARGET_TPS;
  let nextTxTime = Date.now();
  let rrIndex = 0;

  while (running) {
    const now = Date.now();
    if (now < nextTxTime) {
      await new Promise((r) => setTimeout(r, Math.max(1, nextTxTime - now)));
    }
    nextTxTime = Date.now() + txIntervalMs;

    let sender: AccountSlot | null = null;
    for (let i = 0; i < accounts.length; i++) {
      const candidate = accounts[(rrIndex + i) % accounts.length];
      if (!candidate.busy && candidate.balance >= currentGasPrice + 2) {
        sender = candidate;
        rrIndex = (rrIndex + i + 1) % accounts.length;
        break;
      }
    }

    if (!sender) {
      await new Promise((r) => setTimeout(r, 50));
      continue;
    }

    sender.busy = true;
    const recipient = pickRandomOther(accounts, sender);
    const amount = Math.floor(Math.random() * 3) + 1;

    const senderRef = sender;
    sendTransaction(senderRef, recipient, amount)
      .then((sent) => {
        if (!sent) {
          senderRef.busy = false;
        }
      })
      .catch(() => {
        senderRef.busy = false;
      });
  }
}

async function main() {
  log("=== Rinku Activity Bot v2 ===");
  log(`Mode: ${MODE}`);
  log(`Nodes: ${NODE_URLS.join(", ")}`);
  log(`Accounts: ${ACCOUNT_COUNT}`);
  log(`Max concurrent HTTP requests: ${MAX_CONCURRENT_REQUESTS}`);
  if (MODE === "stress") log(`Target TPS: ${TARGET_TPS}`);
  log(`Duration: ${RUN_FOREVER ? "forever (until signal)" : `${DURATION_S}s`}`);
  log("");

  log("Connecting WebSockets to all nodes...");
  for (const url of NODE_URLS) {
    wsConnections.push(connectWS(url));
  }
  await new Promise((r) => setTimeout(r, 1500));

  await fetchGasPrice();
  log(`Gas price: ${currentGasPrice.toFixed(4)} RKU`);

  log(`Creating and funding ${ACCOUNT_COUNT} accounts...`);
  const batchSize = 25;
  for (let i = 0; i < ACCOUNT_COUNT; i += batchSize) {
    const batch = [];
    for (let j = i; j < Math.min(i + batchSize, ACCOUNT_COUNT); j++) {
      batch.push(
        createAndFundAccount(j).then((slot) => {
          accounts.push(slot);
          log(
            `  Account ${accounts.length}/${ACCOUNT_COUNT}: ${slot.fingerprint.slice(0, 12)}... (bal=${slot.balance}, nonce=${slot.nonce})`
          );
        }).catch((err) => {
          log(`  Account ${j} creation failed: ${err.message}`);
        })
      );
    }
    await Promise.all(batch);
    if (i + batchSize < ACCOUNT_COUNT) {
      await new Promise((r) => setTimeout(r, 1200));
    }
  }

  if (accounts.length < 2) {
    log("ERROR: Need at least 2 funded accounts to run. Exiting.");
    process.exit(1);
  }

  log(`\nReady: ${accounts.length} accounts funded. Starting ${MODE} mode...\n`);

  stats.startTime = Date.now();
  stats.lastReportTime = Date.now();

  let durationTimer: ReturnType<typeof setTimeout> | null = null;
  if (!RUN_FOREVER) {
    durationTimer = setTimeout(() => {
      running = false;
      log("Duration reached, shutting down...");
    }, DURATION_S * 1000);
  } else {
    log("Running continuously — send SIGTERM/SIGINT to stop");
  }

  const statsInterval = setInterval(() => {
    reportStats();
  }, 5000);

  const gasInterval = setInterval(fetchGasPrice, 2000);

  const refillInterval = setInterval(refillLowBalances, 30000);

  process.on("SIGINT", () => {
    log("SIGINT received, shutting down...");
    running = false;
  });

  process.on("SIGTERM", () => {
    log("SIGTERM received, shutting down...");
    running = false;
  });

  if (MODE === "realistic") {
    await runRealisticMode();
  } else {
    await runStressMode();
  }

  if (durationTimer) clearTimeout(durationTimer);
  clearInterval(statsInterval);
  clearInterval(gasInterval);
  clearInterval(refillInterval);

  reportStats();
  log("");
  log("=== FINAL REPORT ===");
  const elapsed = (Date.now() - stats.startTime) / 1000;
  log(`Duration: ${elapsed.toFixed(1)}s`);
  log(
    `Transactions: ${stats.txSent} sent, ${stats.txConfirmed} confirmed, ${stats.txFailed} failed, ${stats.txTimedOut} timed out`
  );
  log(`Overall TPS: ${(stats.txSent / elapsed).toFixed(2)}`);
  log(
    `Confirmation rate: ${((stats.txConfirmed / Math.max(stats.txSent, 1)) * 100).toFixed(1)}%`
  );

  if (stats.latencies.length > 0) {
    log(
      `Latency: p50=${percentile(stats.latencies, 0.5)}ms p90=${percentile(stats.latencies, 0.9)}ms p95=${percentile(stats.latencies, 0.95)}ms p99=${percentile(stats.latencies, 0.99)}ms max=${Math.max(...stats.latencies)}ms`
    );
  }

  log(`\nPer-account TX counts:`);
  for (const acc of accounts.sort((a, b) => b.txCount - a.txCount)) {
    log(
      `  ${acc.fingerprint.slice(0, 12)}... sent=${acc.txCount} bal=${acc.balance.toFixed(2)} nonce=${acc.nonce}`
    );
  }

  for (const ws of wsConnections) {
    ws.close();
  }

  process.exit(0);
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
