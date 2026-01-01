import { createAPI } from './api.js';
import { StateManager } from './state.js';
import { Consensus } from './consensus.js';
import { Mempool } from './mempool.js';
import { Storage, type NodeSnapshot } from './storage.js';
import { PeerSyncService } from './peerSync.js';
import { ContractService } from './contracts.js';
import { hashTransaction, type SignedTransaction } from '@rinku/core';
import { randomBytes } from 'crypto';

const PORT = parseInt(process.env.NODE_PORT || '3001', 10);
const FAUCET_BALANCE = 1000000;
const DATA_DIR = process.env.RINKU_DATA_DIR || '.rinku-data';
const NODE_PEERS = process.env.NODE_PEERS || '';
const NODE_ID = process.env.NODE_ID || randomBytes(8).toString('hex');

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
    
    if (bootstrapped) {
      console.log('Successfully bootstrapped from peer network');
      await saveSnapshot(storage, state, consensus);
    } else {
      console.log('No peers available, creating local genesis...');
      await createGenesis(state, consensus);
      await saveSnapshot(storage, state, consensus);
    }
  } else {
    console.log('Cold start - initializing genesis...');
    state = new StateManager();
    consensus = new Consensus();
    await createGenesis(state, consensus);
    await saveSnapshot(storage, state, consensus);
  }

  const peerSync = new PeerSyncService(state, consensus, NODE_ID);
  const contractService = new ContractService(state);
  
  peers.forEach(peer => {
    peerSync.addPeer(peer);
    console.log(`Added peer: ${peer}`);
  });

  peerSync.onSyncComplete(async () => {
    await saveSnapshot(storage, state, consensus);
  });

  const app = createAPI(state, consensus, mempool, peerSync, contractService, async () => {
    await saveSnapshot(storage, state, consensus);
  });
  
  console.log('Smart contract service enabled');

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
  consensus: Consensus
): Promise<void> {
  const stateJson = state.toJSON() as { accounts: [string, any][]; merkleRoot: string };
  const consensusJson = consensus.toJSON();
  
  const snapshot: NodeSnapshot = {
    version: 1,
    timestamp: Date.now(),
    state: stateJson,
    dag: consensusJson.dag as { nodes: any[]; tips: string[] },
    publicKeys: consensusJson.publicKeys
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
export { createAPI } from './api.js';
