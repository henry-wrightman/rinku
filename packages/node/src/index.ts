import {
  createAPI,
  type TokenomicsServices,
  type ForkServices,
} from "./api.js";
import { StateManager } from "./state.js";
import { Consensus } from "./consensus.js";
import { Mempool } from "./mempool.js";
import { Storage, type NodeSnapshot } from "./storage.js";
import { PeerSyncService } from "./peerSync.js";
import { ContractService } from "./contracts.js";
import { RewardsService } from "./rewards.js";
import { CheckpointService } from "./checkpoint.js";
import { GasService } from "./gas.js";
import {
  EmissionService,
  SlashingService,
  distributeCheckpointReward,
  TOKENOMICS_CONFIG,
  type SlashingServiceDeps,
} from "./tokenomics.js";
import { FinalityMetricsService } from "./finality.js";
import { ValidatorKeyManager } from "./validator-keys.js";
import { ProofSlashingService } from "./proof-slashing.js";
import { GossipService } from "./gossip.js";
import { ForkRemediationService } from "./fork-remediation.js";
import { CryptoPool } from "./crypto-pool.js";
import { TelemetryService } from "./telemetry.js";
import { NodeTui } from "./tui.js";
import {
  hashTransaction,
  type SignedTransaction,
  DEFAULT_CHECKPOINT_CONFIG,
} from "@rinku/core";
import { randomBytes } from "crypto";
import { cpus } from "os";

const CHECKPOINT_INTERVAL_MS = parseInt(
  process.env.CHECKPOINT_INTERVAL_MS || "15000",
  10,
);

const PORT = parseInt(process.env.NODE_PORT || "3001", 10);
const FAUCET_BALANCE = 1000000;
const DATA_DIR = process.env.RINKU_DATA_DIR || ".rinku-data";
const NODE_PEERS = process.env.NODE_PEERS || "";
const NODE_ID = process.env.NODE_ID || randomBytes(8).toString("hex");
const MAX_DAG_NODES = parseInt(process.env.MAX_DAG_NODES || "300", 10);
const MAX_TIPS = parseInt(process.env.MAX_TIPS || "15", 10);
const PRUNE_INTERVAL_MS = parseInt(
  process.env.PRUNE_INTERVAL_MS || "30000",
  10,
);
const SELF_URL = process.env.SELF_URL || "";
const MAX_PEERS = parseInt(process.env.MAX_PEERS || "50", 10);
const DISCOVERY_ENABLED = process.env.DISCOVERY_ENABLED !== "false";
const GOSSIP_ENABLED = process.env.GOSSIP_ENABLED !== "false";
const GOSSIP_INTERVAL_MS = parseInt(
  process.env.GOSSIP_INTERVAL_MS || "200",
  10,
);
const CRYPTO_WORKERS = parseInt(
  process.env.CRYPTO_WORKERS || String(Math.max(2, cpus().length - 1)),
  10,
);
const NODE_TUI = process.env.NODE_TUI === "true";
const NODE_VERSION = "0.1.0";

async function main() {
  console.log("Starting Rinku Node...");
  console.log(`Node ID: ${NODE_ID}`);
  console.log(`Data dir: ${DATA_DIR}`);

  const storage = new Storage(DATA_DIR);
  let state: StateManager;
  let consensus: Consensus;
  const mempool = new Mempool();

  const snapshot = await storage.load();
  const peers = NODE_PEERS
    ? NODE_PEERS.split(",")
        .map((p) => p.trim())
        .filter((p) => p)
    : [];

  if (snapshot) {
    console.log("Restoring from snapshot...");
    state = StateManager.fromJSON(snapshot.state);
    consensus = Consensus.fromJSON({
      dag: snapshot.dag,
      publicKeys: snapshot.publicKeys,
    });
    console.log(
      `Restored: ${snapshot.dag.nodes.length} transactions, ${snapshot.state.accounts.length} accounts`,
    );
  } else if (peers.length > 0) {
    console.log(
      "Cold start with peers - attempting to bootstrap from network...",
    );
    state = new StateManager();
    consensus = new Consensus();

    const bootstrapped = await bootstrapFromPeers(peers, state, consensus);

    if (!bootstrapped) {
      console.log("No peers available, creating local genesis...");
      await createGenesis(state, consensus);
    } else {
      console.log("Successfully bootstrapped from peer network");
    }
  } else {
    console.log("Cold start - initializing genesis...");
    state = new StateManager();
    consensus = new Consensus();
    await createGenesis(state, consensus);
  }

  const cryptoPool = new CryptoPool(CRYPTO_WORKERS);
  consensus.setCryptoPool(cryptoPool);
  console.log(`Crypto pool initialized with ${CRYPTO_WORKERS} worker threads`);

  const rewardsDeps = {
    getDAGNodeByUrl: (url: string) => consensus.getNodeByUrl(url),
    getAccount: (address: string) => state.getAccount(address),
    updateBalance: async (address: string, delta: number) =>
      state.updateBalance(address, delta),
  };

  let rewardsService: RewardsService;
  if (snapshot?.rewards) {
    rewardsService = RewardsService.fromJSON(snapshot.rewards, rewardsDeps);
    console.log("Restored rewards/staking data from snapshot");
  } else {
    rewardsService = new RewardsService(rewardsDeps);
  }

  const peerSync = new PeerSyncService(state, consensus, NODE_ID);

  if (SELF_URL) {
    peerSync.setSelfUrl(SELF_URL);
    console.log(`Self URL: ${SELF_URL}`);
  }

  peerSync.setDiscoveryConfig({
    maxPeers: MAX_PEERS,
    discoveryEnabled: DISCOVERY_ENABLED,
    announceEnabled: true,
  });

  if (DISCOVERY_ENABLED) {
    console.log(`Peer discovery enabled (max ${MAX_PEERS} peers)`);
  }

  let contractService: ContractService;
  if (snapshot?.contracts) {
    contractService = await ContractService.fromJSON(snapshot.contracts, state);
    console.log("Restored contract service from snapshot");
  } else {
    contractService = new ContractService(state);
  }

  let validatorKeyManagerRef: ValidatorKeyManager | null = null;

  const checkpointDeps = {
    getMerkleRoot: () => state.getMerkleRoot(),
    getTipCount: () => consensus.getTips().length,
    getTotalTransactions: () => consensus.getAllNodes().length,
    getValidatorEntries: () =>
      rewardsService.getActiveValidators().map((v) => {
        const pubKey = consensus.getPublicKeys().get(v.staker);
        const blsKey =
          validatorKeyManagerRef?.getAddress() === v.staker
            ? validatorKeyManagerRef?.getBLSPublicKey()
            : undefined;
        return {
          address: v.staker,
          publicKey: pubKey ? Array.from(pubKey) : [],
          blsPublicKey: blsKey ? Array.from(blsKey) : undefined,
          weight: v.amount,
        };
      }),
    getTotalWeight: () => rewardsService.getTotalStaked(),
    getPublicKey: (address: string) => consensus.getPublicKeys().get(address),
    getPrivateKey: () => validatorKeyManagerRef?.getPrivateKey() || undefined,
    getBLSPrivateKey: () =>
      validatorKeyManagerRef?.getBLSPrivateKey() || undefined,
    getBLSPublicKey: () =>
      validatorKeyManagerRef?.getBLSPublicKey() || undefined,
    getNodeAddress: () => validatorKeyManagerRef?.getAddress() || NODE_ID,
    getAllTransactionHashes: () =>
      consensus.getAllNodes().map((n) => n.tx.hash),
    getStateRoot: () => contractService.getStateRoot(),
    getReceiptRoot: () => contractService.getReceiptRoot(),
  };

  let checkpointService: CheckpointService;
  if (snapshot?.checkpoints) {
    checkpointService = CheckpointService.fromJSON(
      snapshot.checkpoints,
      checkpointDeps,
    );
    if (!checkpointService.getGenesisCheckpoint()) {
      await checkpointService.initializeGenesis();
    }
    console.log("Restored checkpoint data from snapshot");
  } else {
    checkpointService = new CheckpointService(checkpointDeps, "rinku-testnet");
    await checkpointService.initializeGenesis();
  }

  peers.forEach((peer) => {
    peerSync.addPeer(peer);
    console.log(`Added peer: ${peer}`);
  });

  const slashingDeps: SlashingServiceDeps = {
    getStake: (address: string) => {
      const validators = rewardsService.getActiveValidators();
      return validators.find((v) => v.staker === address);
    },
    updateStake: (address: string, newAmount: number) => {
      const validators = rewardsService.getActiveValidators();
      const stake = validators.find((v) => v.staker === address);
      if (stake) {
        stake.amount = newAmount;
      }
    },
    removeStake: (address: string) => {
      const validators = rewardsService.getActiveValidators();
      const idx = validators.findIndex((v) => v.staker === address);
      if (idx >= 0) {
        validators.splice(idx, 1);
      }
    },
    updateBalance: async (address: string, delta: number) =>
      state.updateBalance(address, delta),
  };

  let emissionService: EmissionService;
  let slashingService: SlashingService;

  if (snapshot?.tokenomics?.emission) {
    emissionService = EmissionService.fromJSON(
      snapshot.tokenomics.emission as {
        totalEmitted?: number;
        totalBurned?: number;
      },
    );
    console.log("Restored emission service from snapshot");
  } else {
    emissionService = new EmissionService();
  }

  if (snapshot?.tokenomics?.slashing) {
    slashingService = SlashingService.fromJSON(
      snapshot.tokenomics.slashing as any,
      slashingDeps,
    );
    console.log("Restored slashing service from snapshot");
  } else {
    slashingService = new SlashingService(slashingDeps);
  }

  const tokenomics: TokenomicsServices = {
    emissionService,
    slashingService,
  };

  console.log(
    "Tokenomics service enabled (30M max supply, halving every 3.15M checkpoints ~18mo)",
  );

  let validatorKeyManager: ValidatorKeyManager;
  if (snapshot?.validatorKeys) {
    validatorKeyManager = await ValidatorKeyManager.fromJSON(
      snapshot.validatorKeys as any,
    );
    console.log("Validator key manager initialized from snapshot");
  } else {
    validatorKeyManager = new ValidatorKeyManager();
    await validatorKeyManager.generateNewKey();
    console.log(
      `Generated validator key: ${validatorKeyManager.getAddress()?.slice(0, 16)}...`,
    );
  }

  validatorKeyManagerRef = validatorKeyManager;

  for (const validator of rewardsService.getActiveValidators()) {
    const pubKey = consensus.getPublicKeys().get(validator.staker);
    const blsKey =
      validatorKeyManager.getAddress() === validator.staker
        ? validatorKeyManager.getBLSPublicKey()
        : undefined;
    if (pubKey) {
      validatorKeyManager.registerValidator(
        validator.staker,
        pubKey,
        validator.amount,
        blsKey || undefined,
      );
    }
  }

  const proofSlashingDeps = {
    slashingService,
    keyManager: validatorKeyManager,
    getCurrentCheckpointHeight: () =>
      checkpointService.getLatestCheckpoint()?.height || 0,
    getCheckpointConfig: () => DEFAULT_CHECKPOINT_CONFIG,
  };

  let proofSlashingService: ProofSlashingService;
  if (snapshot?.proofSlashing) {
    proofSlashingService = ProofSlashingService.fromJSON(
      snapshot.proofSlashing as any,
      proofSlashingDeps,
    );
    console.log("Restored proof slashing service from snapshot");
  } else {
    proofSlashingService = new ProofSlashingService(proofSlashingDeps);
  }

  console.log("Validator key management and proof slashing enabled");

  const gossipService = new GossipService(consensus, state, peerSync, NODE_ID, {
    gossipIntervalMs: GOSSIP_INTERVAL_MS,
  });

  let forkRemediationService: ForkRemediationService;
  if (snapshot?.forkRemediation) {
    forkRemediationService = ForkRemediationService.fromJSON(
      snapshot.forkRemediation,
      consensus,
      state,
    );
    console.log("Restored fork remediation service from snapshot");
  } else {
    forkRemediationService = new ForkRemediationService(consensus, state);
  }

  forkRemediationService.setGossipService(gossipService);

  gossipService.setTxReceivedCallback(async (tx, publicKey) => {
    if (consensus.hasTransaction(tx.hash)) return false;

    if (publicKey) {
      consensus.registerPublicKey(tx.from, publicKey);
    }

    const skipValidation = tx.from === "genesis" || tx.from === "faucet";
    if (!skipValidation) {
      const validation = await consensus.validateTransaction(
        tx,
        state.getAllAccounts(),
        publicKey,
      );
      if (!validation.valid) {
        return false;
      }
    }

    await state.applyTransaction(tx, { skipChecks: skipValidation });
    await consensus.addTransaction(tx);
    forkRemediationService.indexTransaction(tx);

    return true;
  });

  gossipService.setCheckpointQuorumCallback(
    async (checkpointId, signatures) => {
      console.log(
        `Checkpoint ${checkpointId.slice(0, 16)}... reached quorum with ${signatures.size} signatures`,
      );
    },
  );

  const forkServices: ForkServices = {
    gossipService,
    forkRemediationService,
  };

  if (GOSSIP_ENABLED) {
    gossipService.start();
    forkRemediationService.start();
    console.log(
      `Gossip and fork remediation enabled (interval: ${GOSSIP_INTERVAL_MS}ms)`,
    );
  }

  let gasService: GasService;
  if (snapshot?.gas) {
    gasService = GasService.fromJSON(snapshot.gas);
    console.log("Restored gas service data from snapshot");
  } else {
    gasService = new GasService({
      minFee: 0.001,
      maxFee: 100,
      baseFee: 0.01,
      burnPercent: 50,
      validatorPercent: 50,
    });
  }

  let finalityMetrics: FinalityMetricsService;
  if (snapshot?.finality) {
    finalityMetrics = FinalityMetricsService.fromJSON(snapshot.finality);
    console.log("Restored finality metrics from snapshot");
  } else {
    finalityMetrics = new FinalityMetricsService();
  }
  finalityMetrics.setCheckpointInterval(CHECKPOINT_INTERVAL_MS);

  peerSync.onSyncComplete(async () => {
    await saveSnapshot(
      storage,
      state,
      consensus,
      rewardsService,
      checkpointService,
      gasService,
      tokenomics,
      finalityMetrics,
      contractService,
      validatorKeyManager,
      proofSlashingService,
      gossipService,
      forkRemediationService,
    );
  });

  let snapshotPending = false;
  let lastSnapshotTime = Date.now();
  const SNAPSHOT_DEBOUNCE_MS = 30000;

  const debouncedSave = async () => {
    const now = Date.now();
    if (now - lastSnapshotTime < SNAPSHOT_DEBOUNCE_MS) {
      snapshotPending = true;
      return;
    }
    snapshotPending = false;
    lastSnapshotTime = now;
    await saveSnapshot(
      storage,
      state,
      consensus,
      rewardsService,
      checkpointService,
      gasService,
      tokenomics,
      finalityMetrics,
      contractService,
      validatorKeyManager,
      proofSlashingService,
      gossipService,
      forkRemediationService,
    );
  };

  const app = createAPI(
    state,
    consensus,
    mempool,
    peerSync,
    contractService,
    rewardsService,
    checkpointService,
    gasService,
    debouncedSave,
    tokenomics,
    finalityMetrics,
    forkServices,
  );

  await saveSnapshot(
    storage,
    state,
    consensus,
    rewardsService,
    checkpointService,
    gasService,
    tokenomics,
    finalityMetrics,
    contractService,
    validatorKeyManager,
    proofSlashingService,
    gossipService,
    forkRemediationService,
  );

  setInterval(async () => {
    if (snapshotPending) {
      snapshotPending = false;
      lastSnapshotTime = Date.now();
      await saveSnapshot(
        storage,
        state,
        consensus,
        rewardsService,
        checkpointService,
        gasService,
        tokenomics,
        finalityMetrics,
        contractService,
        validatorKeyManager,
        proofSlashingService,
        gossipService,
        forkRemediationService,
      );
    }
  }, SNAPSHOT_DEBOUNCE_MS);

  checkpointService.onCheckpoint(async (checkpointId, height) => {
    contractService.setCheckpointHeight(height);

    const checkpoint = checkpointService.getCheckpoint(checkpointId);
    const checkpointTimestamp = checkpoint?.timestamp || Date.now();
    const allNodes = consensus.getAllNodes();
    // const nodeCount = allNodes.length;

    const unfinalizedNodes = allNodes.filter((n) => !n.finality);

    const count = consensus.stampFinalityForAll(checkpointId, height);
    if (count > 0) {
      console.log(
        `[Finality] Stamped ${count} transactions with checkpoint ${checkpointId.slice(0, 8)}... at height ${height}`,
      );
      for (const node of unfinalizedNodes) {
        if (node.tx.hash) {
          finalityMetrics.recordTxFinalized(node.tx.hash, checkpointTimestamp);
        }
      }

      let tipRewardsCount = 0;
      let witnessRewardsCount = 0;
      for (const node of unfinalizedNodes) {
        if (node.url && node.tx.tipUrls && node.tx.tipUrls.length > 0) {
          const rewards = rewardsService.processTransactionRewards(
            node.url,
            node.tx.tipUrls,
            node.tx.amount || 0,
          );
          tipRewardsCount += rewards.tipRewards.length;
          witnessRewardsCount += rewards.witnessRewards.length;
        }
      }
      if (tipRewardsCount > 0 || witnessRewardsCount > 0) {
        console.log(
          `[Rewards] Processed ${tipRewardsCount} tip rewards, ${witnessRewardsCount} witness rewards at height ${height}`,
        );
      }
    }

    finalityMetrics.recordCheckpoint(height, count, checkpointTimestamp);
    finalityMetrics.pruneStaleEntries(height);

    const reward = emissionService.getCheckpointReward(height);
    if (reward > 0 && emissionService.getRemainingToEmit() > 0) {
      const validators = rewardsService.getActiveValidators();
      const validatorWeights = validators.map((v) => {
        const account = state.getAccount(v.staker);
        const ageMs = account ? Date.now() - account.firstTxTimestamp : 0;
        const ageDays = Math.max(0, ageMs / (24 * 60 * 60 * 1000));
        return {
          address: v.staker,
          stakeAmount: v.amount,
          ageWeight: ageDays,
          missedCheckpoints: 0,
        };
      });

      const distribution = distributeCheckpointReward(reward, validatorWeights);
      let distributed = 0;
      for (const [address, amount] of distribution) {
        await state.updateBalance(address, amount);
        distributed += amount;
      }

      if (distributed > 0) {
        emissionService.recordEmission(distributed);
        console.log(
          `[Emission] Distributed ${distributed.toFixed(4)} RKU to ${distribution.size} validators at height ${height}`,
        );
      }
    }

    await slashingService.processUnbondingQueue();
  });

  checkpointService.start(CHECKPOINT_INTERVAL_MS);

  console.log("Smart contract service enabled");
  console.log("Rewards & staking service enabled");
  console.log(
    `Checkpoint service enabled (${CHECKPOINT_INTERVAL_MS / 1000}s interval)`,
  );
  console.log("Gas service enabled (min: 0.001, max: 100)");

  setInterval(async () => {
    consensus.updateWeights(state.getAllAccounts());
    await state.updateMerkleRootIfNeeded();

    const dagStats = consensus.getDAGStats();

    if (dagStats.tips > MAX_TIPS) {
      console.log(
        `[TipWarning] High tip count: ${dagStats.tips} (max: ${MAX_TIPS}). Transactions should reference more tips to consolidate.`,
      );
    }

    const dagSize = consensus.getDAGSize();
    if (dagSize > MAX_DAG_NODES) {
      const pruned = consensus.pruneDAG(MAX_DAG_NODES);
      if (pruned > 0) {
        console.log(
          `[Pruning] Removed ${pruned} old DAG nodes. Size: ${dagSize} -> ${consensus.getDAGSize()}`,
        );
      }
    }

    if (rewardsService) {
      const rewardsPruned = rewardsService.pruneOldData();
      if (rewardsPruned > 0) {
        console.log(
          `[Pruning] Removed ${rewardsPruned} expired witness entries`,
        );
      }
    }

    const memUsage = process.memoryUsage();
    const heapMB = Math.round(memUsage.heapUsed / 1024 / 1024);
    const rewardsStats = rewardsService?.getStats();
    const updatedDagStats = consensus.getDAGStats();
    console.log(
      `[Stats] DAG: ${updatedDagStats.nodes} nodes, Tips: ${updatedDagStats.tips}, Accounts: ${state.getAllAccounts().size}, Heap: ${heapMB} MB, Witnessed: ${rewardsStats?.witnessedCount || 0}`,
    );

    if (heapMB > 300 && typeof global.gc === "function") {
      console.log("[GC] Forcing garbage collection...");
      global.gc();
    }
  }, PRUNE_INTERVAL_MS);
  console.log(
    `DAG pruning enabled (max: ${MAX_DAG_NODES} nodes, max tips: ${MAX_TIPS}, interval: ${PRUNE_INTERVAL_MS / 1000}s)`,
  );

  if (peers.length > 0) {
    peerSync.start(5000);
  }

  const telemetry = new TelemetryService(DATA_DIR);

  if (NODE_TUI && process.stdin.isTTY) {
    const tui = new NodeTui({
      consensus,
      state,
      gossip: gossipService || null,
      peerSync,
      gas: gasService || null,
      telemetry,
      getCheckpointHeight: () =>
        checkpointService?.getLatestCheckpoint()?.height ?? 0,
      getPendingCount: () => mempool.size(),
      getWitnessedCount: () => rewardsService?.getStats().witnessedCount ?? 0,
      getCryptoWorkers: () => cryptoPool.getWorkerCount(),
      setCryptoWorkers: (count: number) => {
        cryptoPool.resize(count).then(() => {
          console.log(
            `[Config] Crypto workers resized to ${cryptoPool.getWorkerCount()}`,
          );
        });
      },
      nodeId: NODE_ID,
      version: NODE_VERSION,
    });
    tui.start();
    console.log("TUI dashboard enabled");
  }

  app.listen(PORT, "0.0.0.0", () => {
    console.log(`Rinku Node running on port ${PORT}`);
    console.log(`API available at http://0.0.0.0:${PORT}/api`);
  });
}

async function createGenesis(
  state: StateManager,
  consensus: Consensus,
): Promise<void> {
  const genesisTx: SignedTransaction = {
    from: "genesis",
    to: "faucet",
    amount: FAUCET_BALANCE,
    fee: 0,
    nonce: 0,
    tipUrls: [],
    sig: "genesis-signature",
    ts: Date.now(),
    hash: "",
  };
  genesisTx.hash = await hashTransaction(genesisTx);

  await state.applyTransaction(genesisTx);
  await consensus.addTransaction(genesisTx);
  consensus.updateWeights(state.getAllAccounts());

  console.log(`Genesis transaction created: ${genesisTx.hash.slice(0, 16)}...`);
  console.log(`Faucet account initialized with ${FAUCET_BALANCE} RKU`);
}

async function bootstrapFromPeers(
  peers: string[],
  state: StateManager,
  consensus: Consensus,
): Promise<boolean> {
  for (const peer of peers) {
    try {
      console.log(`Attempting bootstrap from ${peer}...`);

      const response = await fetch(`${peer}/api/sync/transactions`, {
        signal: AbortSignal.timeout(10000),
      });

      if (!response.ok) continue;

      const data = (await response.json()) as {
        transactions: { tx: SignedTransaction; publicKey?: number[] }[];
      };

      if (data.transactions.length === 0) continue;

      const sorted = data.transactions.sort((a, b) => a.tx.ts - b.tx.ts);

      for (const { tx, publicKey } of sorted) {
        if (publicKey) {
          consensus.registerPublicKey(tx.from, new Uint8Array(publicKey));
        }
        await state.applyTransaction(tx, { skipChecks: true });
        await consensus.addTransaction(tx);
      }

      consensus.updateWeights(state.getAllAccounts());
      console.log(
        `Bootstrapped ${data.transactions.length} transactions from ${peer}`,
      );
      return true;
    } catch (err) {
      console.log(`Failed to bootstrap from ${peer}: ${err}`);
    }
  }
  return false;
}

async function saveSnapshot(
  storage: Storage,
  state: StateManager,
  consensus: Consensus,
  rewardsService?: RewardsService,
  checkpointService?: CheckpointService,
  gasService?: GasService,
  tokenomics?: TokenomicsServices,
  finalityMetrics?: FinalityMetricsService,
  contractService?: ContractService,
  keyManager?: ValidatorKeyManager,
  proofSlashing?: ProofSlashingService,
  gossipService?: GossipService,
  forkRemediationService?: ForkRemediationService,
): Promise<void> {
  const stateJson = state.toJSON() as {
    accounts: [string, any][];
    merkleRoot: string;
  };
  const consensusJson = consensus.toJSON();

  const snapshot: NodeSnapshot = {
    version: 1,
    timestamp: Date.now(),
    state: stateJson,
    dag: consensusJson.dag as { nodes: any[]; tips: string[] },
    publicKeys: consensusJson.publicKeys,
    rewards: rewardsService?.toJSON(),
    checkpoints: checkpointService?.toJSON(),
    gas: gasService?.toJSON(),
    tokenomics: {
      emission: tokenomics?.emissionService?.toJSON(),
      slashing: tokenomics?.slashingService?.toJSON(),
    },
    finality: finalityMetrics?.toJSON(),
    contracts: contractService?.toJSON() as NodeSnapshot["contracts"],
    validatorKeys: keyManager?.toJSON(),
    proofSlashing: proofSlashing?.toJSON(),
    gossip: gossipService?.toJSON(),
    forkRemediation: forkRemediationService?.toJSON(),
  };

  await storage.save(snapshot);
}

main().catch(console.error);

export { StateManager } from "./state.js";
export { Consensus } from "./consensus.js";
export { Mempool } from "./mempool.js";
export { Storage } from "./storage.js";
export { PeerSyncService } from "./peerSync.js";
export { ContractService } from "./contracts.js";
export { RewardsService } from "./rewards.js";
export { CheckpointService } from "./checkpoint.js";
export { GasService } from "./gas.js";
export {
  EmissionService,
  SlashingService,
  TOKENOMICS_CONFIG,
  distributeCheckpointReward,
} from "./tokenomics.js";
export { ValidatorKeyManager } from "./validator-keys.js";
export { ProofSlashingService } from "./proof-slashing.js";
export { GossipService } from "./gossip.js";
export { ForkRemediationService } from "./fork-remediation.js";
export { createAPI } from "./api.js";
export { TelemetryService } from "./telemetry.js";
export { NodeTui } from "./tui.js";
