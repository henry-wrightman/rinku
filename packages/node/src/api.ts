import express from 'express';
import cors from 'cors';
import { StateManager } from './state.js';
import { Consensus } from './consensus.js';
import { Mempool } from './mempool.js';
import { PeerSyncService } from './peerSync.js';
import { parseTransactionURL, type SignedTransaction } from '@rinku/core';

export function createAPI(
  state: StateManager,
  consensus: Consensus,
  mempool: Mempool,
  peerSync?: PeerSyncService,
  onTransaction?: () => Promise<void>
) {
  const app = express();

  app.use(cors());
  app.use(express.json({ limit: '10mb' }));

  app.get('/api/health', (_req, res) => {
    res.json({ status: 'ok', timestamp: Date.now() });
  });

  app.get('/api/tips', (_req, res) => {
    const tips = consensus.getTips();
    res.json({ tips });
  });

  app.get('/api/account/:fingerprint', (req, res) => {
    const account = state.getAccount(req.params.fingerprint);
    if (!account) {
      res.status(404).json({ error: 'Account not found' });
      return;
    }
    res.json(account);
  });

  app.get('/api/accounts', (_req, res) => {
    const accounts = Array.from(state.getAllAccounts().entries()).map(
      ([fp, account]) => ({
        fingerprint: fp,
        balance: account.balance,
        nonce: account.nonce,
        firstTxTimestamp: account.firstTxTimestamp
      })
    );
    res.json({ accounts });
  });

  app.post('/api/tx', async (req, res) => {
    try {
      const { tx, publicKey } = req.body as {
        tx: SignedTransaction;
        publicKey?: number[];
      };

      if (!tx) {
        res.status(400).json({ error: 'Transaction required' });
        return;
      }

      const pubKeyArray = publicKey ? new Uint8Array(publicKey) : undefined;
      
      if (pubKeyArray) {
        consensus.registerPublicKey(tx.from, pubKeyArray);
      }

      const validation = await consensus.validateTransaction(
        tx,
        state.getAllAccounts(),
        pubKeyArray
      );

      if (!validation.valid) {
        res.status(400).json({ error: validation.error });
        return;
      }

      const applied = await state.applyTransaction(tx);
      if (!applied) {
        res.status(400).json({ error: 'Failed to apply transaction' });
        return;
      }

      await consensus.addTransaction(tx);
      consensus.updateWeights(state.getAllAccounts());

      if (onTransaction) {
        await onTransaction();
      }

      res.json({
        success: true,
        hash: tx.hash,
        merkleRoot: state.getMerkleRoot()
      });
    } catch (error: any) {
      res.status(500).json({ error: error.message });
    }
  });

  app.get('/api/tx/:hash', (req, res) => {
    const node = consensus.getNode(req.params.hash);
    if (!node) {
      res.status(404).json({ error: 'Transaction not found' });
      return;
    }
    res.json(node);
  });

  app.get('/api/dag', (_req, res) => {
    const nodes = consensus.getAllNodes();
    const tips = consensus.getTips();
    res.json({ nodes, tips, merkleRoot: state.getMerkleRoot() });
  });

  app.get('/api/state', (_req, res) => {
    res.json({
      accounts: Array.from(state.getAllAccounts().entries()),
      merkleRoot: state.getMerkleRoot(),
      tips: consensus.getTips()
    });
  });

  app.get('/tx/:payload', (req, res) => {
    const url = `/tx/${req.params.payload}`;
    const tx = parseTransactionURL(url);
    if (!tx) {
      res.status(400).json({ error: 'Invalid transaction URL' });
      return;
    }
    res.json(tx);
  });

  app.get('/api/sync/status', (_req, res) => {
    if (!peerSync) {
      res.status(501).json({ error: 'Peer sync not enabled' });
      return;
    }
    res.json(peerSync.getStatus());
  });

  app.get('/api/sync/transactions', (_req, res) => {
    const nodes = consensus.getAllNodes();
    const publicKeys = consensus.getPublicKeys();
    
    const transactions = nodes.map(node => ({
      tx: node.tx,
      publicKey: publicKeys.has(node.tx.from) 
        ? Array.from(publicKeys.get(node.tx.from)!)
        : undefined
    }));

    res.json({ transactions });
  });

  app.get('/api/sync/peers', (_req, res) => {
    if (!peerSync) {
      res.status(501).json({ error: 'Peer sync not enabled' });
      return;
    }
    res.json({ peers: peerSync.getPeers() });
  });

  app.post('/api/sync/force', async (_req, res) => {
    if (!peerSync) {
      res.status(501).json({ error: 'Peer sync not enabled' });
      return;
    }
    
    try {
      const result = await peerSync.forceSync();
      res.json(result);
    } catch (error: any) {
      res.status(500).json({ error: error.message });
    }
  });

  return app;
}
