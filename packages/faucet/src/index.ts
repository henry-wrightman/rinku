import express from 'express';
import cors from 'cors';
import {
  generateKeyPair,
  hashTransaction,
  sign,
  arrayToHex,
  type SignedTransaction
} from '@rinku/core';

const PORT = parseInt(process.env.FAUCET_PORT || '3002', 10);
const NODE_URL = process.env.NODE_URL || 'http://localhost:3001';
const FAUCET_AMOUNT = 100;
const RATE_LIMIT_MS = 60000;

const requestLog = new Map<string, number>();

async function main() {
  console.log('Starting Rinku Faucet...');

  const app = express();
  app.use(cors());
  app.use(express.json());

  app.get('/api/health', (_req, res) => {
    res.json({ status: 'ok', service: 'faucet' });
  });

  app.post('/api/request', async (req, res) => {
    try {
      const { address, publicKey } = req.body;

      if (!address) {
        res.status(400).json({ error: 'Address required' });
        return;
      }

      const lastRequest = requestLog.get(address);
      const now = Date.now();

      if (lastRequest && now - lastRequest < RATE_LIMIT_MS) {
        const waitTime = Math.ceil((RATE_LIMIT_MS - (now - lastRequest)) / 1000);
        res.status(429).json({ 
          error: `Rate limited. Try again in ${waitTime} seconds` 
        });
        return;
      }

      const tipsResponse = await fetch(`${NODE_URL}/api/tips`);
      const tipsData = await tipsResponse.json() as { tips: string[] };
      const tips = tipsData.tips.length > 0 ? tipsData.tips.slice(0, 2) : ['genesis'];

      const faucetResponse = await fetch(`${NODE_URL}/api/account/faucet`);
      const faucetAccount = await faucetResponse.json() as { nonce?: number };

      const tx: SignedTransaction = {
        from: 'faucet',
        to: address,
        amount: FAUCET_AMOUNT,
        nonce: (faucetAccount.nonce || 0) + 1,
        tips,
        sig: 'faucet-signature',
        ts: now,
        hash: ''
      };

      tx.hash = await hashTransaction(tx);

      const submitResponse = await fetch(`${NODE_URL}/api/tx`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ tx })
      });

      if (!submitResponse.ok) {
        const error = await submitResponse.json() as { error?: string };
        res.status(400).json({ error: error.error || 'Faucet request failed' });
        return;
      }

      requestLog.set(address, now);

      res.json({
        success: true,
        amount: FAUCET_AMOUNT,
        txHash: tx.hash
      });
    } catch (error: any) {
      console.error('Faucet error:', error);
      res.status(500).json({ error: error.message });
    }
  });

  app.listen(PORT, '0.0.0.0', () => {
    console.log(`Rinku Faucet running on port ${PORT}`);
    console.log(`Connected to node at ${NODE_URL}`);
  });
}

main().catch(console.error);
