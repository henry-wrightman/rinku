import { Wallet } from "@rinku/wallet";

const NODE_URL = process.env.RINKU_NODE_URL || "http://localhost:3001";
const FAUCET_URL = process.env.RINKU_FAUCET_URL || "http://localhost:3001";

const TX_INTERVAL_MS = parseInt(process.env.TX_INTERVAL || "50");
const MAX_WALLETS = parseInt(process.env.MAX_WALLETS || "20");
const CONCURRENT_TX_COUNT = parseInt(process.env.CONCURRENT_TX || "10");
const FETCH_TIMEOUT_MS = 5000;
const FAUCET_COOLDOWN_MS = 61000;

const globalLockedSenders = new Set<string>();
const localNonces = new Map<string, number>();

interface BotWallet {
  wallet: Wallet;
  fingerprint: string;
  lastFaucetHit: number;
}

interface FastPathStatus {
  tx_hash: string;
  status: string;
  aggregated_stake: number;
  quorum_threshold: number;
  quorum_percent: number;
  ack_count: number;
  finality_time_ms: number | null;
  confirmed_at: string | null;
}

const wallets: BotWallet[] = [];
let totalFastPathTx = 0;
let totalConfirmed = 0;
let totalPending = 0;
let errors = 0;
let pendingOperations = 0;
const maxPendingOps = 100;

let totalFinalityTimeMs = 0;
let finalityCount = 0;
const recentFinalities: number[] = [];

function log(msg: string): void {
  console.log(`[${new Date().toISOString()}] ${msg}`);
}

function lockSender(fingerprint: string): boolean {
  if (globalLockedSenders.has(fingerprint)) return false;
  globalLockedSenders.add(fingerprint);
  return true;
}

function unlockSender(fingerprint: string): void {
  globalLockedSenders.delete(fingerprint);
}

function getNextNonce(fingerprint: string): number | undefined {
  return localNonces.get(fingerprint);
}

function incrementLocalNonce(fingerprint: string): void {
  const current = localNonces.get(fingerprint) ?? 0;
  localNonces.set(fingerprint, current + 1);
}

function parseExpectedNonce(errorMsg: string): number | null {
  const match = errorMsg.match(/expected\s+(\d+)/i);
  return match ? parseInt(match[1], 10) : null;
}

function updateNonceFromError(fingerprint: string, errorMsg: string): boolean {
  const expectedNonce = parseExpectedNonce(errorMsg);
  if (expectedNonce !== null) {
    localNonces.set(fingerprint, expectedNonce);
    return true;
  }
  return false;
}

function fetchWithTimeout(
  url: string,
  options: RequestInit = {},
  timeoutMs: number = FETCH_TIMEOUT_MS,
): Promise<Response> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  return fetch(url, { ...options, signal: controller.signal }).finally(() =>
    clearTimeout(timeout),
  );
}

async function faucetRequest(fingerprint: string): Promise<boolean> {
  try {
    const res = await fetchWithTimeout(`${FAUCET_URL}/api/request`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ address: fingerprint }),
    });
    return res.ok;
  } catch {
    return false;
  }
}

async function createNewWallet(): Promise<void> {
  if (wallets.length >= MAX_WALLETS) return;
  if (pendingOperations >= maxPendingOps) return;

  pendingOperations++;
  try {
    const wallet = new Wallet(NODE_URL);
    const fingerprint = await wallet.create();
    const success = await faucetRequest(fingerprint);

    if (success) {
      localNonces.set(fingerprint, 0);
      wallets.push({
        wallet,
        fingerprint,
        lastFaucetHit: Date.now(),
      });
      log(`New wallet: ${fingerprint.slice(0, 16)}... (${wallets.length} total)`);
    }
  } finally {
    pendingOperations--;
  }
}

function pickRandom<T>(arr: T[]): T {
  return arr[Math.floor(Math.random() * arr.length)];
}

function generateMemo(): string {
  const messages = [
    "fast-path-test",
    "data-only-tx",
    "sub-second-finality",
    `ts:${Date.now()}`,
    `rand:${Math.random().toString(36).slice(2, 10)}`,
  ];
  return pickRandom(messages);
}

async function getTips(): Promise<string[]> {
  try {
    const res = await fetchWithTimeout(`${NODE_URL}/api/dag/tips?limit=3`, {}, 3000);
    if (res.ok) {
      const data = await res.json() as { tips: string[] };
      return data.tips || [];
    }
  } catch {}
  return [];
}

async function checkFastPathStatus(txHash: string): Promise<FastPathStatus | null> {
  try {
    const res = await fetchWithTimeout(`${NODE_URL}/api/tx/fast/${txHash}`, {}, 3000);
    if (res.ok) {
      return await res.json() as FastPathStatus;
    }
  } catch {}
  return null;
}

async function sendFastPathTransaction(sender: BotWallet): Promise<string | null> {
  const nonce = getNextNonce(sender.fingerprint);
  if (nonce === undefined) {
    localNonces.set(sender.fingerprint, 0);
    return null;
  }

  try {
    const tips = await getTips();
    const memo = generateMemo();

    const signedTx = await sender.wallet.createSignedTransactionWithOptions({
      to: sender.fingerprint,
      amount: 0,
      fee: 0.01,
      tipUrls: tips,
      nonce,
    });

    const txWithReferences = {
      ...signedTx,
      memo,
      references: tips.length > 0 ? tips : undefined,
    };

    const publicKey = await sender.wallet.getPublicKey();
    const res = await fetchWithTimeout(`${NODE_URL}/api/tx`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ tx: txWithReferences, publicKey: Array.from(publicKey) }),
    });

    if (res.ok) {
      incrementLocalNonce(sender.fingerprint);
      totalFastPathTx++;
      const data = await res.json() as { hash: string; fast_path_eligible?: boolean };
      if (data.fast_path_eligible) {
        return data.hash;
      }
      return data.hash;
    } else {
      const errData = (await res.json().catch(() => ({}))) as { error?: string };
      const errMsg = errData.error || "";
      if (errMsg.toLowerCase().includes("nonce")) {
        updateNonceFromError(sender.fingerprint, errMsg);
      }
      errors++;
      return null;
    }
  } catch {
    errors++;
    return null;
  }
}

const pendingTxHashes: Map<string, number> = new Map();

async function doFastPathBurst(): Promise<void> {
  if (wallets.length < 1) return;
  if (pendingOperations >= maxPendingOps) return;

  pendingOperations++;
  const lockedForThisBatch: string[] = [];

  try {
    const txCount = Math.min(CONCURRENT_TX_COUNT, wallets.length);
    const txPromises: Promise<string | null>[] = [];

    for (let i = 0; i < txCount; i++) {
      const availableSenders = wallets.filter(
        (w) => !globalLockedSenders.has(w.fingerprint),
      );
      if (availableSenders.length < 1) break;

      const sender = pickRandom(availableSenders);
      if (!lockSender(sender.fingerprint)) continue;
      lockedForThisBatch.push(sender.fingerprint);

      txPromises.push(
        sendFastPathTransaction(sender)
          .catch(() => null)
          .finally(() => unlockSender(sender.fingerprint)),
      );
    }

    if (txPromises.length > 0) {
      const results = await Promise.all(txPromises);
      const successHashes = results.filter((h): h is string => h !== null);
      
      for (const hash of successHashes) {
        pendingTxHashes.set(hash, Date.now());
      }
    }
  } finally {
    for (const fp of lockedForThisBatch) {
      unlockSender(fp);
    }
    pendingOperations--;
  }
}

async function checkPendingConfirmations(): Promise<void> {
  const now = Date.now();
  const toRemove: string[] = [];

  for (const [hash, submitTime] of pendingTxHashes.entries()) {
    if (now - submitTime > 10000) {
      toRemove.push(hash);
      continue;
    }

    const status = await checkFastPathStatus(hash);
    if (status) {
      if (status.status === "confirmed") {
        totalConfirmed++;
        if (status.finality_time_ms !== null) {
          totalFinalityTimeMs += status.finality_time_ms;
          finalityCount++;
          recentFinalities.push(status.finality_time_ms);
          if (recentFinalities.length > 100) recentFinalities.shift();
        }
        toRemove.push(hash);
      } else if (status.status === "pending") {
        totalPending++;
      }
    }
  }

  for (const hash of toRemove) {
    pendingTxHashes.delete(hash);
  }
}

function printStats(): void {
  const avgFinality = finalityCount > 0 ? totalFinalityTimeMs / finalityCount : 0;
  const recentAvg = recentFinalities.length > 0 
    ? recentFinalities.reduce((a, b) => a + b, 0) / recentFinalities.length 
    : 0;
  const minRecent = recentFinalities.length > 0 ? Math.min(...recentFinalities) : 0;
  const maxRecent = recentFinalities.length > 0 ? Math.max(...recentFinalities) : 0;

  console.log("\n" + "=".repeat(60));
  console.log("FAST-PATH BOT STATISTICS");
  console.log("=".repeat(60));
  console.log(`Wallets: ${wallets.length}/${MAX_WALLETS}`);
  console.log(`Total fast-path TX submitted: ${totalFastPathTx}`);
  console.log(`Confirmed (quorum reached): ${totalConfirmed}`);
  console.log(`Pending in queue: ${pendingTxHashes.size}`);
  console.log(`Errors: ${errors}`);
  console.log("-".repeat(60));
  console.log(`Average finality time: ${avgFinality.toFixed(1)}ms`);
  console.log(`Recent avg (last ${recentFinalities.length}): ${recentAvg.toFixed(1)}ms`);
  console.log(`Recent range: ${minRecent.toFixed(0)}ms - ${maxRecent.toFixed(0)}ms`);
  console.log(`TPS estimate: ~${(totalFastPathTx / ((Date.now() - startTime) / 1000)).toFixed(1)} tx/s`);
  console.log("=".repeat(60) + "\n");
}

async function refillWallets(): Promise<void> {
  const now = Date.now();
  for (const wallet of wallets) {
    if (now - wallet.lastFaucetHit >= FAUCET_COOLDOWN_MS) {
      const success = await faucetRequest(wallet.fingerprint);
      if (success) {
        wallet.lastFaucetHit = now;
      }
    }
  }
}

let startTime = Date.now();

async function main() {
  console.log("=".repeat(60));
  console.log("RINKU FAST-PATH BOT - SUB-SECOND FINALITY TEST");
  console.log("=".repeat(60));
  console.log(`Node: ${NODE_URL}`);
  console.log(`TX interval: ${TX_INTERVAL_MS}ms`);
  console.log(`Concurrent TX per burst: ${CONCURRENT_TX_COUNT}`);
  console.log(`Max wallets: ${MAX_WALLETS}`);
  console.log("=".repeat(60) + "\n");

  log("Creating initial wallets...");
  for (let i = 0; i < Math.min(5, MAX_WALLETS); i++) {
    await createNewWallet();
    await new Promise((r) => setTimeout(r, 500));
  }

  startTime = Date.now();

  setInterval(async () => {
    if (wallets.length < MAX_WALLETS && Math.random() < 0.1) {
      await createNewWallet();
    }
  }, 10000);

  setInterval(async () => {
    await doFastPathBurst();
  }, TX_INTERVAL_MS);

  setInterval(async () => {
    await checkPendingConfirmations();
  }, 200);

  setInterval(printStats, 30000);

  setInterval(async () => {
    await refillWallets();
  }, 30000);

  log("Fast-path bot running!");
  log("Sending 0-amount + memo transactions for sub-second finality...");
  log("Press Ctrl+C to stop.");
}

main().catch(console.error);
