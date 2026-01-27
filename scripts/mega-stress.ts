#!/usr/bin/env tsx
import { Wallet } from "@rinku/wallet";
import { sign, hashTransaction } from "@rinku/core";

const NODE_URL = process.env.RINKU_NODE_URL || "http://localhost:3001";
const FAUCET_URL = process.env.RINKU_FAUCET_URL || "http://localhost:3002";

const TOTAL_TRANSACTIONS = parseInt(process.env.TOTAL_TXS || "1000000");
const WALLET_COUNT = parseInt(process.env.WALLET_COUNT || "100");
const BATCH_SIZE = parseInt(process.env.BATCH_SIZE || "50");
const CONCURRENT_BATCHES = parseInt(process.env.CONCURRENT_BATCHES || "10");
const INTER_BATCH_DELAY_MS = parseInt(process.env.INTER_BATCH_DELAY_MS || "20");
const FETCH_TIMEOUT_MS = parseInt(process.env.FETCH_TIMEOUT_MS || "30000");
const FAUCET_AMOUNT = 100000;

interface StressWallet {
  wallet: Wallet;
  fingerprint: string;
  nonce: number;
  balance: number;
  keyPair: any;
}

interface BatchTxItem {
  tx: {
    from: string;
    to: string;
    amount: number;
    nonce: number;
    ts: number;
    parents: string[];
    kind?: string;
    fee: number;
    sig: string;
    hash: string;
  };
}

interface Stats {
  submitted: number;
  successful: number;
  failed: number;
  backpressure: number;
  startTime: number;
  batchesSent: number;
  lastLogTime: number;
  lastLogSuccessful: number;
}

const stats: Stats = {
  submitted: 0,
  successful: 0,
  failed: 0,
  backpressure: 0,
  startTime: 0,
  batchesSent: 0,
  lastLogTime: 0,
  lastLogSuccessful: 0,
};

const wallets: StressWallet[] = [];
let cachedTips: string[] = [];
let lastTipFetch = 0;
const TIP_CACHE_MS = 200;

function log(msg: string): void {
  const now = new Date().toISOString();
  console.log(`[${now}] ${msg}`);
}

function fetchWithTimeout(
  url: string,
  options: RequestInit = {},
  timeoutMs: number = FETCH_TIMEOUT_MS
): Promise<Response> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  return fetch(url, { ...options, signal: controller.signal }).finally(() =>
    clearTimeout(timeout)
  );
}

async function getTips(): Promise<string[]> {
  const now = Date.now();
  if (now - lastTipFetch < TIP_CACHE_MS && cachedTips.length > 0) {
    return cachedTips;
  }
  try {
    const res = await fetchWithTimeout(`${NODE_URL}/api/tips`, {}, 3000);
    if (res.ok) {
      const data = (await res.json()) as { tips: string[] };
      cachedTips = (data.tips || []).slice(0, 4).map((h) => `rinku://tx/h/${h}`);
      lastTipFetch = now;
    }
  } catch {
    // Use cached
  }
  return cachedTips.length > 0 ? cachedTips : [];
}

async function createWallets(): Promise<void> {
  log(`Creating ${WALLET_COUNT} wallets...`);
  const batchSize = 20;

  for (let i = 0; i < WALLET_COUNT; i += batchSize) {
    const batch = Math.min(batchSize, WALLET_COUNT - i);
    const createPromises: Promise<void>[] = [];

    for (let j = 0; j < batch; j++) {
      createPromises.push(
        (async () => {
          const wallet = new Wallet(NODE_URL);
          const fingerprint = await wallet.create();
          const keyPair = (wallet as any).keyManager.getKeyPair();
          wallets.push({
            wallet,
            fingerprint,
            nonce: 0,
            balance: 0,
            keyPair,
          });
        })()
      );
    }
    await Promise.all(createPromises);
  }
  log(`Created ${wallets.length} wallets`);
}

async function fundWallet(w: StressWallet): Promise<boolean> {
  try {
    const res = await fetchWithTimeout(
      `${FAUCET_URL}/api/request`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ address: w.fingerprint }),
      },
      10000
    );
    if (res.ok) {
      w.balance = FAUCET_AMOUNT;
      return true;
    }
  } catch (e) {
    // silent
  }
  return false;
}

async function fundAllWallets(): Promise<void> {
  log(`Funding ${wallets.length} wallets via faucet...`);
  const BATCH = 10;
  let funded = 0;

  for (let i = 0; i < wallets.length; i += BATCH) {
    const batch = wallets.slice(i, i + BATCH);
    const results = await Promise.all(batch.map(fundWallet));
    funded += results.filter(Boolean).length;
    if ((i + BATCH) % 50 === 0 || i + BATCH >= wallets.length) {
      log(`Funded ${funded}/${wallets.length} wallets`);
    }
    await new Promise((r) => setTimeout(r, 100));
  }

  log(`Funding complete: ${funded}/${wallets.length} wallets funded`);
}

async function refreshNonces(): Promise<void> {
  log("Refreshing nonces from chain...");
  const BATCH = 20;

  for (let i = 0; i < wallets.length; i += BATCH) {
    const batch = wallets.slice(i, i + BATCH);
    await Promise.all(
      batch.map(async (w) => {
        try {
          const state = await w.wallet.refresh();
          w.nonce = state.nonce;
          w.balance = state.balance;
        } catch {}
      })
    );
  }
  log("Nonces refreshed");
}

async function prepareBatch(batchIndex: number): Promise<BatchTxItem[]> {
  const tips = await getTips();
  const items: BatchTxItem[] = [];
  const ts = Date.now();

  for (let i = 0; i < BATCH_SIZE; i++) {
    const senderIdx = (batchIndex * BATCH_SIZE + i) % wallets.length;
    const receiverIdx = (senderIdx + 1) % wallets.length;
    const sender = wallets[senderIdx];
    const receiver = wallets[receiverIdx];

    const amount = 0.001;
    const fee = 0.01;
    const nonce = sender.nonce;

    const txForHash = {
      from: sender.fingerprint,
      to: receiver.fingerprint,
      amount,
      fee,
      nonce,
      tipUrls: tips,
      sig: "",
      ts,
    };

    const hash = await hashTransaction(txForHash);
    const sig = await sign(hash, sender.keyPair.privateKey);

    sender.nonce++;

    items.push({
      tx: {
        from: sender.fingerprint,
        to: receiver.fingerprint,
        amount,
        nonce,
        ts,
        parents: tips,
        fee,
        sig,
        hash,
      },
    });
  }

  return items;
}

async function submitBatch(batch: BatchTxItem[]): Promise<{
  successful: number;
  failed: number;
  backpressure: boolean;
}> {
  try {
    const res = await fetchWithTimeout(
      `${NODE_URL}/api/tx/batch`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ transactions: batch }),
      },
      FETCH_TIMEOUT_MS
    );

    if (res.status === 503) {
      return { successful: 0, failed: batch.length, backpressure: true };
    }

    if (res.ok) {
      const data = (await res.json()) as {
        successful: number;
        failed: number;
        total: number;
      };
      return {
        successful: data.successful,
        failed: data.failed,
        backpressure: false,
      };
    } else {
      const text = await res.text();
      if (text.includes("System under load")) {
        return { successful: 0, failed: batch.length, backpressure: true };
      }
      return { successful: 0, failed: batch.length, backpressure: false };
    }
  } catch (e) {
    return { successful: 0, failed: batch.length, backpressure: false };
  }
}

function printProgress(): void {
  const now = Date.now();
  const elapsed = (now - stats.startTime) / 1000;
  const windowElapsed = (now - stats.lastLogTime) / 1000;
  const windowTps =
    windowElapsed > 0
      ? (stats.successful - stats.lastLogSuccessful) / windowElapsed
      : 0;
  const avgTps = elapsed > 0 ? stats.successful / elapsed : 0;
  const progress = ((stats.submitted / TOTAL_TRANSACTIONS) * 100).toFixed(1);

  log(
    `Progress: ${progress}% | Submitted: ${stats.submitted.toLocaleString()} | Success: ${stats.successful.toLocaleString()} | Failed: ${stats.failed.toLocaleString()} | BP: ${stats.backpressure} | Current TPS: ${windowTps.toFixed(0)} | Avg TPS: ${avgTps.toFixed(0)}`
  );

  stats.lastLogTime = now;
  stats.lastLogSuccessful = stats.successful;
}

async function runStressTest(): Promise<void> {
  log("╔════════════════════════════════════════════════════════════════╗");
  log("║                    RINKU MEGA STRESS TEST                       ║");
  log("╚════════════════════════════════════════════════════════════════╝");
  log("");
  log(`Target: ${TOTAL_TRANSACTIONS.toLocaleString()} transactions`);
  log(`Wallets: ${WALLET_COUNT}`);
  log(`Batch size: ${BATCH_SIZE} txs`);
  log(`Concurrent batches: ${CONCURRENT_BATCHES}`);
  log(`Theoretical max batch rate: ${BATCH_SIZE * CONCURRENT_BATCHES} txs/cycle`);
  log(`Inter-batch delay: ${INTER_BATCH_DELAY_MS}ms`);
  log(`Node URL: ${NODE_URL}`);
  log(`Faucet URL: ${FAUCET_URL}`);
  log("");

  await createWallets();
  await fundAllWallets();

  log("Waiting for transactions to be finalized...");
  await new Promise((r) => setTimeout(r, 3000));
  await refreshNonces();

  log("");
  log("=== STARTING STRESS TEST ===");
  stats.startTime = Date.now();
  stats.lastLogTime = stats.startTime;

  let batchIndex = 0;
  const progressInterval = setInterval(printProgress, 5000);

  while (stats.submitted < TOTAL_TRANSACTIONS) {
    const batchPromises: Promise<void>[] = [];

    for (
      let i = 0;
      i < CONCURRENT_BATCHES && stats.submitted + i * BATCH_SIZE < TOTAL_TRANSACTIONS;
      i++
    ) {
      const currentBatchIndex = batchIndex++;
      batchPromises.push(
        (async () => {
          try {
            const batch = await prepareBatch(currentBatchIndex);
            const result = await submitBatch(batch);

            stats.submitted += batch.length;
            stats.successful += result.successful;
            stats.failed += result.failed;
            stats.batchesSent++;

            if (result.backpressure) {
              stats.backpressure++;
              await new Promise((r) => setTimeout(r, 2000));
            }
          } catch (e) {
            stats.failed += BATCH_SIZE;
          }
        })()
      );
    }

    await Promise.all(batchPromises);

    if (INTER_BATCH_DELAY_MS > 0) {
      await new Promise((r) => setTimeout(r, INTER_BATCH_DELAY_MS));
    }
  }

  clearInterval(progressInterval);
  printProgress();

  const totalTime = (Date.now() - stats.startTime) / 1000;
  const finalTps = stats.successful / totalTime;

  log("");
  log("╔════════════════════════════════════════════════════════════════╗");
  log("║                    STRESS TEST COMPLETE                        ║");
  log("╚════════════════════════════════════════════════════════════════╝");
  log("");
  log(`Total time: ${totalTime.toFixed(1)} seconds`);
  log(`Transactions submitted: ${stats.submitted.toLocaleString()}`);
  log(`Successful: ${stats.successful.toLocaleString()}`);
  log(`Failed: ${stats.failed.toLocaleString()}`);
  log(`Backpressure events: ${stats.backpressure}`);
  log(`Batches sent: ${stats.batchesSent.toLocaleString()}`);
  log("");
  log(`═══ RESULTS ═══`);
  log(`Average TPS: ${finalTps.toFixed(1)}`);
  log(`Submit rate: ${(stats.submitted / totalTime).toFixed(1)} txs/sec`);
  log(`Success rate: ${((stats.successful / stats.submitted) * 100).toFixed(2)}%`);
}

async function healthCheck(): Promise<boolean> {
  try {
    const res = await fetchWithTimeout(`${NODE_URL}/health`, {}, 5000);
    return res.ok;
  } catch {
    return false;
  }
}

async function main(): Promise<void> {
  log("Checking node health...");
  const healthy = await healthCheck();
  if (!healthy) {
    log("ERROR: Node is not responding at " + NODE_URL);
    log("Make sure the Rinku Rust Node workflow is running.");
    process.exit(1);
  }
  log("Node is healthy");

  try {
    await runStressTest();
  } catch (e) {
    log(`Fatal error: ${e}`);
    process.exit(1);
  }
}

main();
