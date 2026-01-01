import express from 'express';
import cors from 'cors';
import { StateManager } from './state.js';
import { Consensus } from './consensus.js';
import { Mempool } from './mempool.js';
import { PeerSyncService } from './peerSync.js';
import { parseTransactionURL, createTransactionURL, type SignedTransaction } from '@rinku/core';

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
    const nodes = consensus.getAllNodes().map(node => ({
      ...node,
      url: createTransactionURL(node.tx).path
    }));
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

  app.get('/tx/:payload', async (req, res) => {
    const url = `/tx/${req.params.payload}`;
    const tx = parseTransactionURL(url) as SignedTransaction | null;
    if (!tx) {
      res.status(400).json({ error: 'Invalid transaction URL' });
      return;
    }

    const existingNode = consensus.getNode(tx.hash);
    if (existingNode) {
      res.json({
        status: 'exists',
        message: 'Transaction already in DAG',
        tx: existingNode.tx,
        url
      });
      return;
    }

    const submit = req.query.submit === 'true' || req.query.submit === '1';
    
    if (!submit) {
      res.json({
        status: 'preview',
        message: 'Add ?submit=true&pubkey=<base64> to submit this transaction',
        tx,
        url
      });
      return;
    }

    try {
      let pubKeyArray: Uint8Array | undefined;
      
      if (typeof req.query.pubkey === 'string') {
        try {
          const pubkeyB64 = req.query.pubkey.replace(/-/g, '+').replace(/_/g, '/');
          pubKeyArray = new Uint8Array(Buffer.from(pubkeyB64, 'base64'));
          consensus.registerPublicKey(tx.from, pubKeyArray);
        } catch {
        }
      }

      const skipValidation = tx.from === 'genesis' || tx.from === 'faucet';
      
      if (!skipValidation) {
        if (!pubKeyArray) {
          res.status(400).json({ 
            status: 'invalid',
            error: 'Public key required for signature verification. Add &pubkey=<base64url> to the URL.',
            tx,
            url
          });
          return;
        }
        
        const validation = await consensus.validateTransaction(
          tx,
          state.getAllAccounts(),
          pubKeyArray
        );

        if (!validation.valid) {
          res.status(400).json({ 
            status: 'invalid',
            error: validation.error,
            tx,
            url
          });
          return;
        }
      }

      const applied = await state.applyTransaction(tx, { skipChecks: skipValidation });
      if (!applied) {
        res.status(400).json({ 
          status: 'failed',
          error: 'Failed to apply transaction',
          tx,
          url
        });
        return;
      }

      await consensus.addTransaction(tx);
      consensus.updateWeights(state.getAllAccounts());

      if (onTransaction) {
        await onTransaction();
      }

      res.json({
        status: 'submitted',
        message: 'Transaction added to DAG',
        hash: tx.hash,
        merkleRoot: state.getMerkleRoot(),
        tx,
        url
      });
    } catch (error: any) {
      res.status(500).json({ 
        status: 'error',
        error: error.message,
        tx,
        url
      });
    }
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
