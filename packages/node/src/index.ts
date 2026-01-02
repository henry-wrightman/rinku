import { createAPI } from './api.js';
import { StateManager } from './state.js';
import { Consensus } from './consensus.js';
import { Mempool } from './mempool.js';
import { Storage, type NodeSnapshot } from './storage.js';
import { PeerSyncService } from './peerSync.js';
import { ContractService } from './contracts.js';
import { RewardsService } from './rewards.js';
import { CheckpointService } from './checkpoint.js';
import { hashTransaction, type SignedTransaction } from '@rinku/core';
import { randomBytes } from 'crypto';

const PORT = parseInt(process.env.NODE_PORT || '3001', 10);
const FAUCET_BALANCE = 1000000;
const DATA_DIR = process.env.RINKU_DATA_DIR || '.rinku-data';
const NODE_PEERS = process.env.NODE_PEERS || '';
const NODE_ID = process.env.NODE_ID || randomBytes(8).toString('hex');
const MAX_DAG_NODES = parseInt(process.env.MAX_DAG_NODES || '300', 10);
const PRUNE_INTERVAL_MS = parseInt(process.env.PRUNE_INTERVAL_MS || '30000', 10);
const SELF_URL = process.env.SELF_URL || '';
const MAX_PEERS = parseInt(process.env.MAX_PEERS || '50', 10);
const DISCOVERY_ENABLED = process.env.DISCOVERY_ENABLED !== 'false';

async function main() {
  console.log('Starting Rinku Node...');
  console.log(`Node ID: ${NODE_ID}`);
  console.log(`Data dir: ${DATA_DIR}`);

  const storage = new Storage(DATA_DIR);
  let state: StateManager;
  let consensus: Consensus;
  const mempool = new Mempool();

  const snapshot = await storage.load();
  const peers = NODE_PEERS ? NODE_PEERS.split(',').map(p => p.trim()).filter(p => p) : [];
  
  if (snapshot) {
    console.log('Restoring from snapshot...');
    state = StateManager.fromJSON(snapshot.state);
    consensus = Consensus.fromJSON({ dag: snapshot.dag, publicKeys: snapshot.publicKeys });
    console.log(`Restored: ${snapshot.dag.nodes.length} transactions, ${snapshot.state.accounts.length} accounts`);
  } else if (peers.length > 0) {
    console.log('Cold start with peers - attempting to bootstrap from network...');
    state = new StateManager();
    consensus = new Consensus();
    
    const bootstrapped = await bootstrapFromPeers(peers, state, consensus);
    
    if (!bootstrapped) {
      console.log('No peers available, creating local genesis...');
      await createGenesis(state, consensus);
    } else {
      console.log('Successfully bootstrapped from peer network');
    }
  } else {
    console.log('Cold start - initializing genesis...');
    state = new StateManager();
    consensus = new Consensus();
    await createGenesis(state, consensus);
  }

  const rewardsDeps = {
    getDAGNodeByUrl: (url: string) => consensus.getNodeByUrl(url),
    getAccount: (address: string) => state.getAccount(address),
    updateBalance: async (address: string, delta: number) => state.updateBalance(address, delta)
  };

  let rewardsService: RewardsService;
  if (snapshot?.rewards) {
    rewardsService = RewardsService.fromJSON(snapshot.rewards, rewardsDeps);
    console.log('Restored rewards/staking data from snapshot');
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
    announceEnabled: true
  });
  
  if (DISCOVERY_ENABLED) {
    console.log(`Peer discovery enabled (max ${MAX_PEERS} peers)`);
  }
  
  const contractService = new ContractService(state);

  const checkpointDeps = {
    getMerkleRoot: () => state.getMerkleRoot(),
    getTipCount: () => consensus.getTips().length,
    getTotalTransactions: () => consensus.getAllNodes().length,
    getValidatorEntries: () => rewardsService.getActiveValidators().map(v => {
      const pubKey = consensus.getPublicKeys().get(v.staker);
      return {
        address: v.staker,
        publicKey: pubKey ? Array.from(pubKey) : [],
        weight: v.amount
      };
    }),
    getTotalWeight: () => rewardsService.getTotalStaked(),
    getPublicKey: (address: string) => consensus.getPublicKeys().get(address),
    getPrivateKey: () => undefined,
    getNodeAddress: () => NODE_ID,
    getAllTransactionHashes: () => consensus.getAllNodes().map(n => n.tx.hash)
  };

  let checkpointService: CheckpointService;
  if (snapshot?.checkpoints) {
    checkpointService = CheckpointService.fromJSON(snapshot.checkpoints, checkpointDeps);
    if (!checkpointService.getGenesisCheckpoint()) {
      await checkpointService.initializeGenesis();
    }
    console.log('Restored checkpoint data from snapshot');
  } else {
    checkpointService = new CheckpointService(checkpointDeps, 'rinku-testnet');
    await checkpointService.initializeGenesis();
  }
  
  peers.forEach(peer => {
    peerSync.addPeer(peer);
    console.log(`Added peer: ${peer}`);
  });

  peerSync.onSyncComplete(async () => {
    await saveSnapshot(storage, state, consensus, rewardsService, checkpointService);
  });

  // Debounced snapshot saving - don't save on every transaction
  let snapshotPending = false;
  let lastSnapshotTime = Date.now();
  const SNAPSHOT_DEBOUNCE_MS = 30000; // Save at most every 30 seconds
  
  const debouncedSave = async () => {
    const now = Date.now();
    if (now - lastSnapshotTime < SNAPSHOT_DEBOUNCE_MS) {
      snapshotPending = true;
      return;
    }
    snapshotPending = false;
    lastSnapshotTime = now;
    await saveSnapshot(storage, state, consensus, rewardsService, checkpointService);
  };
  
  const app = createAPI(state, consensus, mempool, peerSync, contractService, rewardsService, checkpointService, debouncedSave);
  
  await saveSnapshot(storage, state, consensus, rewardsService, checkpointService);
  
  // Periodic snapshot save for pending changes
  setInterval(async () => {
    if (snapshotPending) {
      snapshotPending = false;
      lastSnapshotTime = Date.now();
      await saveSnapshot(storage, state, consensus, rewardsService, checkpointService);
    }
  }, SNAPSHOT_DEBOUNCE_MS);

  checkpointService.onCheckpoint((checkpointId, height) => {
    const count = consensus.stampFinalityForAll(checkpointId, height);
    if (count > 0) {
      console.log(`[Finality] Stamped ${count} transactions with checkpoint ${checkpointId.slice(0, 8)}... at height ${height}`);
    }
  });

  checkpointService.start(60000);
  
  console.log('Smart contract service enabled');
  console.log('Rewards & staking service enabled');
  console.log('Checkpoint service enabled (60s interval)');

  setInterval(async () => {
    consensus.updateWeights(state.getAllAccounts());
    await state.updateMerkleRootIfNeeded();
    
    const dagSize = consensus.getDAGSize();
    if (dagSize > MAX_DAG_NODES) {
      const pruned = consensus.pruneDAG(MAX_DAG_NODES);
      if (pruned > 0) {
        console.log(`[Pruning] Removed ${pruned} old DAG nodes. Size: ${dagSize} -> ${consensus.getDAGSize()}`);
      }
    }
    
    if (rewardsService) {
      const rewardsPruned = rewardsService.pruneOldData();
      if (rewardsPruned > 0) {
        console.log(`[Pruning] Removed ${rewardsPruned} expired witness entries`);
      }
    }
    
    const memUsage = process.memoryUsage();
    const heapMB = Math.round(memUsage.heapUsed / 1024 / 1024);
    const rewardsStats = rewardsService?.getStats();
    const dagStats = consensus.getDAGStats();
    console.log(`[Stats] DAG: ${dagStats.nodes} nodes, Tips: ${dagStats.tips}, Accounts: ${state.getAllAccounts().size}, Heap: ${heapMB} MB, Witnessed: ${rewardsStats?.witnessedCount || 0}`);
    
    if (heapMB > 300 && typeof global.gc === 'function') {
      console.log('[GC] Forcing garbage collection...');
      global.gc();
    }
  }, PRUNE_INTERVAL_MS);
  console.log(`DAG pruning enabled (max: ${MAX_DAG_NODES} nodes, interval: ${PRUNE_INTERVAL_MS / 1000}s)`);

  if (peers.length > 0) {
    peerSync.start(5000);
  }

  app.listen(PORT, '0.0.0.0', () => {
    console.log(`Rinku Node running on port ${PORT}`);
    console.log(`API available at http://0.0.0.0:${PORT}/api`);
  });
}

async function createGenesis(state: StateManager, consensus: Consensus): Promise<void> {
  const genesisTx: SignedTransaction = {
    from: 'genesis',
    to: 'faucet',
    amount: FAUCET_BALANCE,
    nonce: 0,
    tipUrls: [],
    sig: 'genesis-signature',
    ts: Date.now(),
    hash: ''
  };
  genesisTx.hash = await hashTransaction(genesisTx);
  
  await state.applyTransaction(genesisTx);
  await consensus.addTransaction(genesisTx);
  consensus.updateWeights(state.getAllAccounts());
  
  console.log(`Genesis transaction created: ${genesisTx.hash.slice(0, 16)}...`);
  console.log(`Faucet account initialized with ${FAUCET_BALANCE} coins`);
}

async function bootstrapFromPeers(
  peers: string[],
  state: StateManager,
  consensus: Consensus
): Promise<boolean> {
  for (const peer of peers) {
    try {
      console.log(`Attempting bootstrap from ${peer}...`);
      
      const response = await fetch(`${peer}/api/sync/transactions`, {
        signal: AbortSignal.timeout(10000)
      });
      
      if (!response.ok) continue;
      
      const data = await response.json() as {
        transactions: { tx: SignedTransaction; publicKey?: number[] }[]
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
      console.log(`Bootstrapped ${data.transactions.length} transactions from ${peer}`);
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
  checkpointService?: CheckpointService
): Promise<void> {
  const stateJson = state.toJSON() as { accounts: [string, any][]; merkleRoot: string };
  const consensusJson = consensus.toJSON();
  
  const snapshot: NodeSnapshot = {
    version: 1,
    timestamp: Date.now(),
    state: stateJson,
    dag: consensusJson.dag as { nodes: any[]; tips: string[] },
    publicKeys: consensusJson.publicKeys,
    rewards: rewardsService?.toJSON(),
    checkpoints: checkpointService?.toJSON()
  };

  await storage.save(snapshot);
}

main().catch(console.error);

export { StateManager } from './state.js';
export { Consensus } from './consensus.js';
export { Mempool } from './mempool.js';
export { Storage } from './storage.js';
export { PeerSyncService } from './peerSync.js';
export { ContractService } from './contracts.js';
export { RewardsService } from './rewards.js';
export { CheckpointService } from './checkpoint.js';
export { createAPI } from './api.js';
