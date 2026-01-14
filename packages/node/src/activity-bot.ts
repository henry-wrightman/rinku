import { Wallet } from "@rinku/wallet";

const NODE_URL = process.env.RINKU_NODE_URL || "http://localhost:3001";
const FAUCET_URL = process.env.RINKU_FAUCET_URL || "http://localhost:3002";

const FAUCET_INTERVAL_MS = parseInt(process.env.FAUCET_INTERVAL || "60000");
const TX_INTERVAL_MS = parseInt(process.env.TX_INTERVAL || "200"); // Reduced from 2000 for higher throughput
const MAX_WALLETS = parseInt(process.env.MAX_WALLETS || "100");
const FAUCET_COOLDOWN_MS = 61000;
const FETCH_TIMEOUT_MS = 15000;
const CONCURRENT_TX_COUNT = parseInt(process.env.CONCURRENT_TX || "50"); // Increased from 30
const BATCH_TX_COUNT = parseInt(process.env.BATCH_TX_COUNT || "30"); // Increased from 20
const BATCH_TX_CHANCE = parseFloat(process.env.BATCH_TX_CHANCE || "0.9"); // Prefer batch (was 0.6)
const CONTRACT_INTERVAL_MS = parseInt(
  process.env.CONTRACT_INTERVAL || "120000",
);
const STAKING_INTERVAL_MS = parseInt(process.env.STAKING_INTERVAL || "180000");
const REWARDS_INTERVAL_MS = parseInt(process.env.REWARDS_INTERVAL || "300000");
const CONSOLIDATION_INTERVAL_MS = parseInt(
  process.env.CONSOLIDATION_INTERVAL || "10000",
);
const TIP_CONSOLIDATION_THRESHOLD = parseInt(
  process.env.TIP_CONSOLIDATION_THRESHOLD || "10",
);
const CONSOLIDATION_TIP_COUNT = parseInt(
  process.env.CONSOLIDATION_TIP_COUNT || "12",
);

// Gas-aware throttling - pause when gas gets too high
const GAS_THROTTLE_THRESHOLD = parseFloat(
  process.env.GAS_THROTTLE_THRESHOLD || "1000",
); // Pause when gas > 1000 RKU (uncapped for testnet testing)
const GAS_RESUME_THRESHOLD = parseFloat(
  process.env.GAS_RESUME_THRESHOLD || "1",
); // Resume when gas < 1 RKU
let gasThrottled = false;

// Global sender lock - prevents same sender from being used in overlapping operations
const globalLockedSenders = new Set<string>();

// Lock a sender for the duration of a transaction
function lockSender(fingerprint: string): boolean {
  if (globalLockedSenders.has(fingerprint)) return false;
  globalLockedSenders.add(fingerprint);
  return true;
}

function unlockSender(fingerprint: string): void {
  globalLockedSenders.delete(fingerprint);
}

// Cached tips with auto-refresh
let cachedTipUrls: string[] = [];
let lastTipFetch = 0;
const TIP_CACHE_MS = 1000; // Refresh tips every 1s

async function getFreshTips(): Promise<string[]> {
  const now = Date.now();
  if (now - lastTipFetch < TIP_CACHE_MS && cachedTipUrls.length > 0) {
    return cachedTipUrls;
  }
  try {
    const res = await fetchWithTimeout(`${NODE_URL}/api/tipUrls`, {}, 3000);
    if (res.ok) {
      const data = (await res.json()) as { tipUrls: string[] };
      cachedTipUrls = data.tipUrls || [];
      lastTipFetch = now;
    }
  } catch {
    // Use cached tips on error
  }
  return cachedTipUrls;
}

interface BotWallet {
  wallet: Wallet;
  fingerprint: string;
  lastFaucetHit: number;
  isStaking: boolean;
  hasContract: boolean;
}

interface DeployedContract {
  contractId: string;
  ownerFingerprint: string;
  deployedAt: number;
}

const wallets: BotWallet[] = [];
const deployedContracts: DeployedContract[] = [];
let totalFaucetHits = 0;
let totalTransactions = 0;
let totalContractDeploys = 0;
let totalContractCalls = 0;
let totalStakes = 0;
let totalRewardsClaimed = 0;
let totalBatchTransactions = 0;
let totalConsolidations = 0;
let errors = 0;
let pendingOperations = 0;
let maxPendingOps = 50; // Increased from 5 for higher throughput
const MAX_CONTRACTS = 3;

let currentGasPrice = 0.01;
const GAS_PRICE_CACHE_MS = 5000;
let lastGasPriceFetch = 0;
let lastLoggedGasPrice = 0;
let skippedDueToBalance = 0;

async function fetchGasPrice(): Promise<number> {
  const now = Date.now();
  if (now - lastGasPriceFetch < GAS_PRICE_CACHE_MS) {
    return currentGasPrice;
  }
  try {
    const res = await fetchWithTimeout(`${NODE_URL}/api/gas/price`, {}, 3000);
    if (res.ok) {
      const data = (await res.json()) as { current: number };
      if (typeof data.current === "number") {
        currentGasPrice = data.current;
        lastGasPriceFetch = now;

        // Gas-aware throttling
        if (!gasThrottled && currentGasPrice > GAS_THROTTLE_THRESHOLD) {
          gasThrottled = true;
          log(
            `Gas throttle ACTIVATED: ${currentGasPrice.toFixed(4)} > ${GAS_THROTTLE_THRESHOLD} threshold`,
          );
        } else if (gasThrottled && currentGasPrice < GAS_RESUME_THRESHOLD) {
          gasThrottled = false;
          log(
            `Gas throttle RELEASED: ${currentGasPrice.toFixed(4)} < ${GAS_RESUME_THRESHOLD} threshold`,
          );
        }

        // Only log if gas changed by >5%
        if (
          Math.abs(currentGasPrice - lastLoggedGasPrice) /
            Math.max(lastLoggedGasPrice, 0.001) >
          0.05
        ) {
          log(
            `Gas price: ${currentGasPrice.toFixed(4)}${gasThrottled ? " [THROTTLED]" : ""}`,
          );
          lastLoggedGasPrice = currentGasPrice;
        }
      }
    }
  } catch (ex) {
    log(`Failed to fetch gas price, using cached: ${currentGasPrice} ${ex}`);
  }
  return currentGasPrice;
}

function pickRandom<T>(arr: T[]): T {
  return arr[Math.floor(Math.random() * arr.length)];
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
  } catch (err: any) {
    log(`Faucet error: ${err.message}`);
    if (err.name === "AbortError") {
      log(`Faucet timeout for ${fingerprint.slice(0, 8)}...`);
    }
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
      wallets.push({
        wallet,
        fingerprint,
        lastFaucetHit: Date.now(),
        isStaking: false,
        hasContract: false,
      });
      totalFaucetHits++;
      log(
        `New wallet: ${fingerprint.slice(0, 16)}... (${wallets.length} total)`,
      );
    } else {
      log(`Failed to create wallet: faucet request failed`);
      errors++;
    }
  } finally {
    pendingOperations--;
  }
}

async function getAverageWalletBalance(): Promise<number> {
  if (wallets.length === 0) return 0;
  let total = 0;
  for (const w of wallets) {
    try {
      total += await w.wallet.getBalance();
    } catch {
      // Skip failed balance checks
    }
  }
  return total / wallets.length;
}

async function doAggressiveFaucetRefill(): Promise<void> {
  if (wallets.length === 0) return;

  const avgBalance = await getAverageWalletBalance();
  const minNeeded = currentGasPrice + 10; // gas + small tx + buffer

  if (avgBalance >= minNeeded * 2) return; // Wallets have enough

  log(
    `Low balance detected! Avg: ${avgBalance.toFixed(2)}, need: ${minNeeded.toFixed(2)} - refilling wallets...`,
  );

  const now = Date.now();
  const eligible = wallets.filter(
    (w) => now - w.lastFaucetHit >= FAUCET_COOLDOWN_MS,
  );

  if (eligible.length === 0) {
    log(`No wallets eligible for faucet yet (cooldown)`);
    return;
  }

  // Refill up to 3 wallets at once
  const toRefill = eligible.slice(0, Math.min(3, eligible.length));
  let refilled = 0;

  for (const wallet of toRefill) {
    if (pendingOperations >= maxPendingOps) break;
    pendingOperations++;
    try {
      const success = await faucetRequest(wallet.fingerprint);
      if (success) {
        wallet.lastFaucetHit = now;
        totalFaucetHits++;
        refilled++;
      }
    } finally {
      pendingOperations--;
    }
  }

  if (refilled > 0) {
    log(`Emergency refill: ${refilled}/${toRefill.length} wallets topped up`);
  }
}

async function doRandomFaucetDrop(): Promise<void> {
  if (wallets.length === 0) return;
  if (pendingOperations >= maxPendingOps) return;

  const now = Date.now();
  const eligible = wallets.filter(
    (w) => now - w.lastFaucetHit >= FAUCET_COOLDOWN_MS,
  );

  if (eligible.length === 0) return;

  pendingOperations++;
  try {
    const recipient = pickRandom(eligible);
    const success = await faucetRequest(recipient.fingerprint);

    if (success) {
      recipient.lastFaucetHit = now;
      totalFaucetHits++;
      log(`Faucet drop: ${recipient.fingerprint.slice(0, 16)}...`);
    } else {
      log(`Faucet drop failed for ${recipient.fingerprint.slice(0, 16)}...`);
      errors++;
    }
  } finally {
    pendingOperations--;
  }
}

async function doSingleTransaction(
  sender: BotWallet,
  recipient: BotWallet,
  amount: number,
  fee: number,
): Promise<boolean> {
  try {
    await sender.wallet.send(recipient.fingerprint, amount, fee);
    totalTransactions++;
    return true;
  } catch (err: any) {
    log(`Transaction failed: ${err?.message}`);
    errors++;
    return false;
  }
}

async function doConcurrentTransactions(): Promise<void> {
  if (wallets.length < 2) return;
  if (pendingOperations >= maxPendingOps) return;

  const gasPrice = await fetchGasPrice();

  // Skip if gas throttled - let price recover
  if (gasThrottled) return;

  pendingOperations++;
  const lockedForThisBatch: string[] = [];
  try {
    const txPromises: Promise<boolean>[] = [];
    const txCount = Math.min(
      CONCURRENT_TX_COUNT,
      Math.floor(wallets.length / 2),
    );

    for (let i = 0; i < txCount; i++) {
      // Find sender not locked globally
      const availableSenders = wallets.filter(
        (w) => !globalLockedSenders.has(w.fingerprint),
      );
      if (availableSenders.length < 1) break;

      const sender = pickRandom(availableSenders);
      if (!lockSender(sender.fingerprint)) continue; // Race protection
      lockedForThisBatch.push(sender.fingerprint);

      const recipients = wallets.filter(
        (w) => w.fingerprint !== sender.fingerprint,
      );
      if (recipients.length === 0) continue;

      const recipient = pickRandom(recipients);
      const amount = Math.floor(Math.random() * 5) + 1;

      txPromises.push(
        sender.wallet
          .getBalance()
          .then(async (balance) => {
            const needed = amount + gasPrice + 5;
            if (balance < needed) {
              skippedDueToBalance++;
              return false;
            }
            return doSingleTransaction(sender, recipient, amount, gasPrice);
          })
          .catch(() => false)
          .finally(() => unlockSender(sender.fingerprint)),
      );
    }

    if (txPromises.length > 0) {
      const results = await Promise.all(txPromises);
      const successCount = results.filter((r) => r).length;
      if (successCount > 0) {
        log(
          `Concurrent TX batch: ${successCount}/${txPromises.length} succeeded (creating ${successCount} potential tips)`,
        );
      }
    }
  } finally {
    // Unlock any senders that didn't get into a promise
    for (const fp of lockedForThisBatch) {
      unlockSender(fp);
    }
    pendingOperations--;
  }
}

async function doBatchTransactions(): Promise<void> {
  if (wallets.length < 2) return;
  if (pendingOperations >= maxPendingOps) return;

  const gasPrice = await fetchGasPrice();

  // Skip if gas throttled - let price recover
  if (gasThrottled) return;

  pendingOperations++;
  const lockedForThisBatch: string[] = [];
  try {
    const batchCount = Math.min(BATCH_TX_COUNT, Math.floor(wallets.length / 2));

    const preparedTxs: Array<{
      sender: BotWallet;
      recipient: BotWallet;
      amount: number;
      signedTx: any;
      publicKey: number[];
    }> = [];

    for (let i = 0; i < batchCount; i++) {
      // Find sender not locked globally
      const availableSenders = wallets.filter(
        (w) => !globalLockedSenders.has(w.fingerprint),
      );
      if (availableSenders.length < 1) break;

      const sender = pickRandom(availableSenders);
      if (!lockSender(sender.fingerprint)) continue; // Race protection
      lockedForThisBatch.push(sender.fingerprint);

      const recipients = wallets.filter(
        (w) => w.fingerprint !== sender.fingerprint,
      );
      if (recipients.length === 0) continue;

      const recipient = pickRandom(recipients);
      const amount = Math.floor(Math.random() * 5) + 1;

      try {
        const balance = await sender.wallet.getBalance();
        const needed = amount + gasPrice + 5;
        if (balance < needed) {
          skippedDueToBalance++;
          continue;
        }

        const signedTx = await sender.wallet.createSignedTransaction(
          recipient.fingerprint,
          amount,
          gasPrice,
        );

        if (signedTx) {
          const publicKey = await sender.wallet.getPublicKey();
          preparedTxs.push({
            sender,
            recipient,
            amount,
            signedTx,
            publicKey: Array.from(publicKey),
          });
        }
      } catch {
        continue;
      }
    }

    // Unlock senders before submitting batch
    for (const fp of lockedForThisBatch) {
      unlockSender(fp);
    }
    lockedForThisBatch.length = 0;

    if (preparedTxs.length === 0) return;

    const res = await fetchWithTimeout(`${NODE_URL}/api/tx/batch`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        transactions: preparedTxs.map((p) => ({
          tx: p.signedTx,
          publicKey: p.publicKey,
        })),
      }),
    });

    if (res.ok) {
      const result = (await res.json()) as {
        total: number;
        successful: number;
        failed: number;
      };
      totalBatchTransactions += result.successful;
      totalTransactions += result.successful;
      log(
        `Batch TX: ${result.successful}/${result.total} succeeded via batch API`,
      );
    } else {
      const errData = await res.json().catch(() => ({}));
      log(`Batch TX failed: ${JSON.stringify(errData).slice(0, 50)}`);
      errors++;
    }
  } catch (err: any) {
    errors++;
  } finally {
    // Safety cleanup
    for (const fp of lockedForThisBatch) {
      unlockSender(fp);
    }
    pendingOperations--;
  }
}

async function deployContract(): Promise<void> {
  if (wallets.length < 3) return;
  if (pendingOperations >= maxPendingOps) return;
  if (deployedContracts.length >= MAX_CONTRACTS) return;

  const eligible = wallets.filter((w) => !w.hasContract);
  if (eligible.length === 0) return;

  pendingOperations++;
  try {
    const creator = pickRandom(eligible);
    const balance = await creator.wallet.getBalance();
    if (balance < 50) {
      return;
    }

    const wasmBase64 = btoa("mock-token-contract-v1");

    const res = await fetchWithTimeout(`${NODE_URL}/api/contracts/deploy`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        creator: creator.fingerprint,
        wasmBase64,
        initState: { balances: {}, totalSupply: 0 },
      }),
    });

    if (res.ok) {
      const data = (await res.json()) as { contractId: string };
      deployedContracts.push({
        contractId: data.contractId,
        ownerFingerprint: creator.fingerprint,
        deployedAt: Date.now(),
      });
      creator.hasContract = true;
      totalContractDeploys++;
      log(
        `Contract deployed: ${data.contractId.slice(0, 16)}... by ${creator.fingerprint.slice(0, 12)}`,
      );
    } else {
      const errData = await res.json().catch(() => ({}));
      log(`Contract deploy failed: ${JSON.stringify(errData).slice(0, 50)}`);
      errors++;
    }
  } catch (err: any) {
    errors++;
  } finally {
    pendingOperations--;
  }
}

async function callContract(): Promise<void> {
  if (deployedContracts.length === 0) return;
  if (wallets.length < 2) return;
  if (pendingOperations >= maxPendingOps) return;

  pendingOperations++;
  try {
    const contract = pickRandom(deployedContracts);
    const caller = pickRandom(wallets);

    const balance = await caller.wallet.getBalance();
    if (balance < 10) return;

    const action = Math.random();
    let entrypoint: string;
    let input: object;

    if (action < 0.4) {
      entrypoint = "mint";
      input = {
        to: caller.fingerprint,
        amount: Math.floor(Math.random() * 50) + 10,
      };
    } else if (action < 0.7) {
      entrypoint = "transfer";
      const recipient = pickRandom(
        wallets.filter((w) => w.fingerprint !== caller.fingerprint),
      );
      input = {
        from: caller.fingerprint,
        to: recipient?.fingerprint || caller.fingerprint,
        amount: Math.floor(Math.random() * 10) + 1,
      };
    } else {
      entrypoint = "get_balance";
      input = { address: caller.fingerprint };
    }

    const res = await fetchWithTimeout(
      `${NODE_URL}/api/contracts/${contract.contractId}/call`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          caller: caller.fingerprint,
          entrypoint,
          input,
        }),
      },
    );

    if (res.ok) {
      totalContractCalls++;
      log(
        `Contract call: ${entrypoint}() on ${contract.contractId.slice(0, 12)}...`,
      );
    } else {
      const errData = await res.json().catch(() => ({}));
      if (entrypoint !== "get_balance") {
        log(`Contract call failed: ${JSON.stringify(errData).slice(0, 40)}`);
      }
    }
  } catch (err: any) {
    log(`Contract call error: ${err.message}`);
    errors++;
  } finally {
    pendingOperations--;
  }
}

async function doStaking(): Promise<void> {
  if (wallets.length < 5) return;
  if (pendingOperations >= maxPendingOps) return;

  const nonStakers = wallets.filter((w) => !w.isStaking);
  if (nonStakers.length === 0) return;

  pendingOperations++;
  try {
    const staker = pickRandom(nonStakers);
    const balance = await staker.wallet.getBalance();

    if (balance < 200) return;

    const stakeAmount = Math.floor(balance * 0.3);

    const res = await fetchWithTimeout(`${NODE_URL}/api/staking/stake`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        address: staker.fingerprint,
        amount: stakeAmount,
      }),
    });

    if (res.ok) {
      staker.isStaking = true;
      totalStakes++;
      log(
        `Staked: ${staker.fingerprint.slice(0, 16)}... staked ${stakeAmount} RKU`,
      );
    }
  } catch (err: any) {
    log(`Staking error: ${err.message}`);
    errors++;
  } finally {
    pendingOperations--;
  }
}

async function claimRewards(): Promise<void> {
  if (wallets.length === 0) return;
  if (pendingOperations >= maxPendingOps) return;

  pendingOperations++;
  try {
    const claimer = pickRandom(wallets);
    log(`Rewards check: ${claimer.fingerprint.slice(0, 16)}...`);

    const summaryRes = await fetchWithTimeout(
      `${NODE_URL}/api/rewards/${claimer.fingerprint}`,
    );
    if (!summaryRes.ok) {
      log(`Rewards check failed: endpoint returned ${summaryRes.status}`);
      return;
    }

    const summary = (await summaryRes.json()) as { pendingRewards: number };
    if (summary.pendingRewards < 1) {
      log(
        `Rewards check: no pending rewards for ${claimer.fingerprint.slice(0, 16)}...`,
      );
      return;
    }

    log(
      `Rewards found: ${summary.pendingRewards.toFixed(4)} pending for ${claimer.fingerprint.slice(0, 16)}...`,
    );

    const res = await fetchWithTimeout(
      `${NODE_URL}/api/rewards/${claimer.fingerprint}/claim`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
      },
    );

    if (res.ok) {
      const result = (await res.json()) as { success: boolean; amount: number };
      if (result.success) {
        totalRewardsClaimed += result.amount || 0;
        log(
          `Rewards claimed: ${claimer.fingerprint.slice(0, 16)}... received ${result.amount} RKU`,
        );
      } else {
        log(
          `Rewards claim returned success=false for ${claimer.fingerprint.slice(0, 16)}...`,
        );
      }
    } else {
      log(
        `Rewards claim failed: ${res.status} for ${claimer.fingerprint.slice(0, 16)}...`,
      );
    }
  } catch (err: any) {
    log(`Rewards claim error: ${err.message}`);
  } finally {
    pendingOperations--;
  }
}

function log(msg: string): void {
  const time = new Date().toLocaleTimeString();
  console.log(`[${time}] ${msg}`);
}

function printStats(): void {
  const memUsage = process.memoryUsage();
  const heapMB = Math.round(memUsage.heapUsed / 1024 / 1024);
  const stakingWallets = wallets.filter((w) => w.isStaking).length;
  const contractOwners = wallets.filter((w) => w.hasContract).length;

  console.log("\n" + "=".repeat(55));
  console.log("RINKU ACTIVITY BOT - ENHANCED TESTNET SIMULATION");
  console.log("=".repeat(55));
  console.log(
    `  Wallets: ${wallets.length}/${MAX_WALLETS} (${stakingWallets} staking, ${contractOwners} w/contracts)`,
  );
  console.log(
    `  Transactions: ${totalTransactions} (${totalBatchTransactions} batch, ${totalConsolidations} consolidations) | Faucet: ${totalFaucetHits}`,
  );
  console.log(
    `  Contracts: ${totalContractDeploys} deployed, ${totalContractCalls} calls`,
  );
  console.log(
    `  Staking: ${totalStakes} stakes, ${totalRewardsClaimed} rewards claimed`,
  );
  console.log(
    `  Errors: ${errors} | Pending: ${pendingOperations}/${maxPendingOps} | Skipped (low bal): ${skippedDueToBalance}`,
  );
  console.log(
    `  Heap: ${heapMB} MB | Concurrent TX: ${CONCURRENT_TX_COUNT} | Locked: ${globalLockedSenders.size}`,
  );
  console.log(`  Gas Price: ${currentGasPrice.toFixed(4)}`);

  // Show average balance async
  getAverageWalletBalance().then((avg) => {
    const needed = currentGasPrice + 10;
    const status = avg >= needed * 2 ? "✓" : avg >= needed ? "⚠" : "✗";
    console.log(
      `  Avg Balance: ${avg.toFixed(2)} (need ~${needed.toFixed(2)} per tx) ${status}`,
    );
    console.log("=".repeat(55) + "\n");
  });

  skippedDueToBalance = 0; // Reset after display
}

async function checkTipCount(): Promise<void> {
  try {
    const res = await fetchWithTimeout(`${NODE_URL}/api/dag/summary`);
    if (res.ok) {
      const data = (await res.json()) as {
        tipCount: number;
        totalNodes: number;
      };
      log(`DAG Status: ${data.totalNodes} nodes, ${data.tipCount} tips`);
    }
  } catch {}
}

async function doConsolidation(): Promise<void> {
  if (gasThrottled) return;
  if (wallets.length < 2) return;

  try {
    const tips = await getFreshTips();
    if (tips.length < TIP_CONSOLIDATION_THRESHOLD) {
      return;
    }

    log(
      `Tip consolidation triggered: ${tips.length} tips (threshold: ${TIP_CONSOLIDATION_THRESHOLD})`,
    );

    const availableWallets = wallets.filter(
      (w) => !globalLockedSenders.has(w.fingerprint),
    );
    if (availableWallets.length < 2) return;

    const consolidationCount = Math.min(
      3,
      Math.ceil(tips.length / CONSOLIDATION_TIP_COUNT),
    );
    const gasPrice = await fetchGasPrice();

    for (let i = 0; i < consolidationCount; i++) {
      const sender = availableWallets[i % availableWallets.length];
      const receiver = availableWallets[(i + 1) % availableWallets.length];

      if (!lockSender(sender.fingerprint)) continue;

      try {
        const state = await sender.wallet.refresh();
        const fee = gasPrice * 1.2;
        const minBalance = 0.001 + fee;

        if (state.balance < minBalance) {
          unlockSender(sender.fingerprint);
          continue;
        }

        const tipSlice = tips.slice(
          i * CONSOLIDATION_TIP_COUNT,
          (i + 1) * CONSOLIDATION_TIP_COUNT,
        );
        if (tipSlice.length < 3) {
          unlockSender(sender.fingerprint);
          continue;
        }

        await sender.wallet.sendWithCustomTips(
          receiver.fingerprint,
          0.001,
          fee,
          tipSlice,
        );

        totalConsolidations++;
        totalTransactions++;
        log(`Consolidation TX merged ${tipSlice.length} tips`);
      } catch (ex) {
        errors++;
        log(`Consolidation error: ${ex}`);
      } finally {
        unlockSender(sender.fingerprint);
      }
    }
  } catch (ex) {
    errors++;
    log(`Consolidation check failed: ${ex}`);
  }
}

async function main() {
  console.log("=".repeat(55));
  console.log("RINKU ACTIVITY BOT - ENHANCED TESTNET SIMULATION");
  console.log("=".repeat(55));
  console.log(`Node: ${NODE_URL}`);
  console.log(`Faucet: ${FAUCET_URL}`);
  console.log(`Concurrent transactions: ${CONCURRENT_TX_COUNT}`);
  console.log(`TX interval: ${TX_INTERVAL_MS / 1000}s`);
  console.log(`Contract interval: ${CONTRACT_INTERVAL_MS / 1000}s`);
  console.log(`Staking interval: ${STAKING_INTERVAL_MS / 1000}s`);
  console.log(`Rewards interval: ${REWARDS_INTERVAL_MS / 1000}s`);
  console.log(`Max wallets: ${MAX_WALLETS}`);
  console.log(
    `Batch TX: ${BATCH_TX_COUNT} per batch, ${Math.round(BATCH_TX_CHANCE * 100)}% chance`,
  );
  console.log(
    `Tip consolidation: threshold=${TIP_CONSOLIDATION_THRESHOLD}, ${CONSOLIDATION_TIP_COUNT} tips/tx, every ${CONSOLIDATION_INTERVAL_MS / 1000}s`,
  );
  console.log("=".repeat(55) + "\n");

  log("Starting enhanced activity simulation...");
  log("Creating initial wallets...");

  for (let i = 0; i < 3; i++) {
    await createNewWallet();
    await new Promise((r) => setTimeout(r, 1000));
  }

  setInterval(async () => {
    if (Math.random() < 0.6 && wallets.length < MAX_WALLETS) {
      await createNewWallet();
    } else {
      await doRandomFaucetDrop();
    }
  }, FAUCET_INTERVAL_MS);

  setInterval(async () => {
    if (Math.random() < BATCH_TX_CHANCE && wallets.length >= 4) {
      await doBatchTransactions();
    } else {
      await doConcurrentTransactions();
    }
  }, TX_INTERVAL_MS);

  setInterval(async () => {
    if (Math.random() < 0.3) {
      await deployContract();
    } else {
      await callContract();
    }
  }, CONTRACT_INTERVAL_MS);

  setInterval(async () => {
    await doStaking();
  }, STAKING_INTERVAL_MS);

  setInterval(async () => {
    await claimRewards();
  }, REWARDS_INTERVAL_MS);

  setInterval(printStats, 60000);
  setInterval(checkTipCount, 30000);
  setInterval(doConsolidation, CONSOLIDATION_INTERVAL_MS);

  // Check wallet balances and refill if needed every 15s
  setInterval(async () => {
    await doAggressiveFaucetRefill();
  }, 15000);

  log("Enhanced bot running!");
  log("Features: Concurrent TX, Contracts, Staking, Rewards");
  log("Press Ctrl+C to stop.");
}

main().catch(console.error);
