import { createTransaction, hashTransaction, type SignedTransaction } from '@rinku/core';

const NODE_URL = process.env.RINKU_NODE_URL || 'http://localhost:3001';
const FAUCET_URL = process.env.RINKU_FAUCET_URL || 'http://localhost:3002';

const FAUCET_INTERVAL_MS = parseInt(process.env.FAUCET_INTERVAL || '30000');
const TX_INTERVAL_MS = parseInt(process.env.TX_INTERVAL || '15000');
const MAX_WALLETS = parseInt(process.env.MAX_WALLETS || '50');

interface WalletState {
  fingerprint: string;
  balance: number;
  nonce: number;
  createdAt: number;
}

const wallets: WalletState[] = [];
let totalFaucetHits = 0;
let totalTransactions = 0;
let errors = 0;

function randomId(): string {
  return `bot_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 8)}`;
}

function pickRandom<T>(arr: T[]): T {
  return arr[Math.floor(Math.random() * arr.length)];
}

async function getTipUrls(): Promise<string[]> {
  try {
    const res = await fetch(`${NODE_URL}/api/tipUrls`);
    const data = await res.json() as { tipUrls?: string[] };
    return data.tipUrls || [];
  } catch {
    return [];
  }
}

async function faucetRequest(fingerprint: string): Promise<boolean> {
  try {
    const res = await fetch(`${FAUCET_URL}/api/request`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ address: fingerprint })
    });
    return res.ok;
  } catch {
    return false;
  }
}

async function submitTransaction(tx: SignedTransaction): Promise<boolean> {
  try {
    const res = await fetch(`${NODE_URL}/api/transactions`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(tx)
    });
    return res.ok;
  } catch {
    return false;
  }
}

async function createNewWallet(): Promise<void> {
  if (wallets.length >= MAX_WALLETS) return;
  
  const fingerprint = randomId();
  const success = await faucetRequest(fingerprint);
  
  if (success) {
    wallets.push({
      fingerprint,
      balance: 100,
      nonce: 0,
      createdAt: Date.now()
    });
    totalFaucetHits++;
    log(`New wallet: ${fingerprint.slice(0, 20)}... (${wallets.length} total)`);
  } else {
    errors++;
  }
}

async function doRandomTransaction(): Promise<void> {
  const fundedWallets = wallets.filter(w => w.balance >= 10);
  if (fundedWallets.length < 2) return;
  
  const sender = pickRandom(fundedWallets);
  const recipients = wallets.filter(w => w.fingerprint !== sender.fingerprint);
  if (recipients.length === 0) return;
  
  const recipient = pickRandom(recipients);
  const amount = Math.floor(Math.random() * Math.min(sender.balance - 1, 20)) + 1;
  
  const tipUrls = await getTipUrls();
  if (tipUrls.length === 0) return;
  
  const tx = createTransaction({
    from: sender.fingerprint,
    to: recipient.fingerprint,
    amount,
    nonce: sender.nonce + 1,
    tipUrls: tipUrls.slice(0, 2),
    sig: `bot_sig_${Date.now()}`,
    ts: Date.now()
  });
  
  const signedTx: SignedTransaction = {
    ...tx,
    hash: await hashTransaction(tx)
  };
  
  const success = await submitTransaction(signedTx);
  
  if (success) {
    sender.balance -= amount;
    sender.nonce++;
    recipient.balance += amount;
    totalTransactions++;
    log(`TX: ${sender.fingerprint.slice(0, 12)} -> ${recipient.fingerprint.slice(0, 12)} : ${amount} coins`);
  } else {
    errors++;
  }
}

function log(msg: string): void {
  const time = new Date().toLocaleTimeString();
  console.log(`[${time}] ${msg}`);
}

function printStats(): void {
  console.log('\n' + '-'.repeat(50));
  console.log(`ACTIVITY BOT STATS`);
  console.log('-'.repeat(50));
  console.log(`  Wallets: ${wallets.length}/${MAX_WALLETS}`);
  console.log(`  Faucet hits: ${totalFaucetHits}`);
  console.log(`  Transactions: ${totalTransactions}`);
  console.log(`  Errors: ${errors}`);
  console.log(`  Faucet interval: ${FAUCET_INTERVAL_MS / 1000}s`);
  console.log(`  TX interval: ${TX_INTERVAL_MS / 1000}s`);
  console.log('-'.repeat(50) + '\n');
}

async function main() {
  console.log('='.repeat(50));
  console.log('RINKU ACTIVITY BOT');
  console.log('='.repeat(50));
  console.log(`Node: ${NODE_URL}`);
  console.log(`Faucet: ${FAUCET_URL}`);
  console.log(`Faucet interval: ${FAUCET_INTERVAL_MS / 1000}s`);
  console.log(`TX interval: ${TX_INTERVAL_MS / 1000}s`);
  console.log(`Max wallets: ${MAX_WALLETS}`);
  console.log('='.repeat(50) + '\n');
  
  log('Starting activity simulation...');
  
  for (let i = 0; i < 5; i++) {
    await createNewWallet();
    await new Promise(r => setTimeout(r, 500));
  }
  
  setInterval(async () => {
    if (Math.random() < 0.7 && wallets.length < MAX_WALLETS) {
      await createNewWallet();
    }
  }, FAUCET_INTERVAL_MS);
  
  setInterval(async () => {
    await doRandomTransaction();
  }, TX_INTERVAL_MS);
  
  setInterval(printStats, 60000);
  
  log('Bot running. Press Ctrl+C to stop.');
}

main().catch(console.error);
