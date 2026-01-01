import express from 'express';
import cors from 'cors';
import { StateManager } from './state.js';
import { Consensus } from './consensus.js';
import { Mempool } from './mempool.js';
import { parseTransactionURL, type SignedTransaction } from '@rinku/core';

export function createAPI(
  state: StateManager,
  consensus: Consensus,
  mempool: Mempool
) {
  const app = express();

  app.use(cors());
  app.use(express.json());

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
      ([fingerprint, account]) => ({
        fingerprint,
        ...account
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

  return app;
}
