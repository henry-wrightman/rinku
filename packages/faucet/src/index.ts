import express from 'express';
import cors from 'cors';
import {
  hashTransaction,
  type SignedTransaction
} from '@rinku/core';

const PORT = parseInt(process.env.FAUCET_PORT || '3002', 10);
const NODE_URL = process.env.NODE_URL || 'http://localhost:3001';
const FAUCET_AMOUNT = 1000;
const RATE_LIMIT_MS = 60000;
const FETCH_TIMEOUT_MS = 10000;
const CLEANUP_INTERVAL_MS = 300000;
const MAX_LOG_ENTRIES = 10000;

const requestLog = new Map<string, number>();

function fetchWithTimeout(url: string, options: RequestInit = {}, timeoutMs: number = FETCH_TIMEOUT_MS): Promise<Response> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  
  return fetch(url, { ...options, signal: controller.signal })
    .finally(() => clearTimeout(timeout));
}

function cleanupRequestLog(): void {
  const now = Date.now();
  const expiredThreshold = now - RATE_LIMIT_MS * 2;
  let cleaned = 0;
  
  for (const [address, timestamp] of requestLog) {
    if (timestamp < expiredThreshold) {
      requestLog.delete(address);
      cleaned++;
    }
  }
  
  if (requestLog.size > MAX_LOG_ENTRIES) {
    const entries = Array.from(requestLog.entries())
      .sort((a, b) => a[1] - b[1]);
    const toRemove = entries.slice(0, entries.length - MAX_LOG_ENTRIES);
    for (const [address] of toRemove) {
      requestLog.delete(address);
      cleaned++;
    }
  }
  
  if (cleaned > 0) {
    console.log(`[Cleanup] Removed ${cleaned} expired rate limit entries. Current size: ${requestLog.size}`);
  }
}

async function main() {
  console.log('Starting Rinku Faucet...');

  const app = express();
  app.use(cors());
  app.use(express.json());

  setInterval(cleanupRequestLog, CLEANUP_INTERVAL_MS);
  console.log(`Rate limit cleanup scheduled every ${CLEANUP_INTERVAL_MS / 1000}s`);

  app.get('/api/health', (_req, res) => {
    res.json({ 
      status: 'ok', 
      service: 'faucet',
      rateLimitEntries: requestLog.size
    });
  });

  app.get('/api/stats', async (_req, res) => {
    try {
      const faucetRes = await fetchWithTimeout(`${NODE_URL}/api/account/faucet`);
      const faucetAccount = await faucetRes.json() as { balance?: number };
      
      res.json({
        rateLimitEntries: requestLog.size,
        maxEntries: MAX_LOG_ENTRIES,
        nodeUrl: NODE_URL,
        genesisAllocation: 1000000,
        currentBalance: faucetAccount.balance || 0,
        totalDistributed: 1000000 - (faucetAccount.balance || 0),
        dropAmount: FAUCET_AMOUNT
      });
    } catch {
      res.json({
        rateLimitEntries: requestLog.size,
        maxEntries: MAX_LOG_ENTRIES,
        nodeUrl: NODE_URL,
        genesisAllocation: 1000000,
        dropAmount: FAUCET_AMOUNT
      });
    }
  });

  app.post('/api/request', async (req, res) => {
    try {
      const { address } = req.body;

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

      const tipUrlsResponse = await fetchWithTimeout(`${NODE_URL}/api/tipUrls`);
      const tipUrlsData = await tipUrlsResponse.json() as { tipUrls: string[] };
      const tipUrls = tipUrlsData.tipUrls.length > 0 ? tipUrlsData.tipUrls.slice(0, 2) : [];

      const faucetResponse = await fetchWithTimeout(`${NODE_URL}/api/account/faucet`);
      const faucetAccount = await faucetResponse.json() as { nonce?: number };

      const tx: SignedTransaction = {
        from: 'faucet',
        to: address,
        amount: FAUCET_AMOUNT,
        fee: 0,
        nonce: (faucetAccount.nonce || 0) + 1,
        tipUrls,
        sig: 'faucet-signature',
        ts: now,
        hash: ''
      };

      tx.hash = await hashTransaction(tx);

      const submitResponse = await fetchWithTimeout(`${NODE_URL}/api/tx`, {
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
      if (error.name === 'AbortError') {
        console.error('Faucet error: Node request timeout');
        res.status(504).json({ error: 'Node request timeout. Please try again.' });
      } else {
        console.error('Faucet error:', error.message);
        res.status(500).json({ error: error.message });
      }
    }
  });

  app.listen(PORT, '0.0.0.0', () => {
    console.log(`Rinku Faucet running on port ${PORT}`);
    console.log(`Connected to node at ${NODE_URL}`);
    console.log(`Fetch timeout: ${FETCH_TIMEOUT_MS}ms`);
  });
}

main().catch(console.error);
