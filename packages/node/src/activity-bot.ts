import { Wallet } from '@rinku/wallet';

const NODE_URL = process.env.RINKU_NODE_URL || 'http://localhost:3001';
const FAUCET_URL = process.env.RINKU_FAUCET_URL || 'http://localhost:3002';

const FAUCET_INTERVAL_MS = parseInt(process.env.FAUCET_INTERVAL || '30000');
const TX_INTERVAL_MS = parseInt(process.env.TX_INTERVAL || '15000');
const MAX_WALLETS = parseInt(process.env.MAX_WALLETS || '50');
const FAUCET_COOLDOWN_MS = 61000;

interface BotWallet {
  wallet: Wallet;
  fingerprint: string;
  lastFaucetHit: number;
}

const wallets: BotWallet[] = [];
let totalFaucetHits = 0;
let totalTransactions = 0;
let errors = 0;

function pickRandom<T>(arr: T[]): T {
  return arr[Math.floor(Math.random() * arr.length)];
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

async function createNewWallet(): Promise<void> {
  if (wallets.length >= MAX_WALLETS) return;
  
  const wallet = new Wallet(NODE_URL);
  const fingerprint = await wallet.create();
  
  const success = await faucetRequest(fingerprint);
  
  if (success) {
    wallets.push({
      wallet,
      fingerprint,
      lastFaucetHit: Date.now()
    });
    totalFaucetHits++;
    log(`New wallet: ${fingerprint.slice(0, 16)}... (${wallets.length} total)`);
  } else {
    errors++;
  }
}

async function doRandomFaucetDrop(): Promise<void> {
  if (wallets.length === 0) return;
  
  const now = Date.now();
  const eligible = wallets.filter(w => now - w.lastFaucetHit >= FAUCET_COOLDOWN_MS);
  
  if (eligible.length === 0) return;
  
  const recipient = pickRandom(eligible);
  const success = await faucetRequest(recipient.fingerprint);
  
  if (success) {
    recipient.lastFaucetHit = now;
    totalFaucetHits++;
    log(`Faucet drop: ${recipient.fingerprint.slice(0, 16)}... (+100 coins)`);
  } else {
    errors++;
  }
}

async function doRandomTransaction(): Promise<void> {
  if (wallets.length < 2) return;
  
  const sender = pickRandom(wallets);
  
  try {
    const balance = await sender.wallet.getBalance();
    if (balance < 10) return;
    
    const recipients = wallets.filter(w => w.fingerprint !== sender.fingerprint);
    if (recipients.length === 0) return;
    
    const recipient = pickRandom(recipients);
    const amount = Math.floor(Math.random() * Math.min(balance - 1, 20)) + 1;
    
    const result = await sender.wallet.send(recipient.fingerprint, amount);
    totalTransactions++;
    log(`TX: ${sender.fingerprint.slice(0, 12)} -> ${recipient.fingerprint.slice(0, 12)} : ${amount} coins`);
  } catch (err: any) {
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
  log('Creating initial wallets with real keypairs...');
  
  for (let i = 0; i < 5; i++) {
    await createNewWallet();
    await new Promise(r => setTimeout(r, 500));
  }
  
  setInterval(async () => {
    if (Math.random() < 0.7 && wallets.length < MAX_WALLETS) {
      await createNewWallet();
    } else {
      await doRandomFaucetDrop();
    }
  }, FAUCET_INTERVAL_MS);
  
  setInterval(async () => {
    await doRandomTransaction();
  }, TX_INTERVAL_MS);
  
  setInterval(printStats, 60000);
  
  log('Bot running with real wallet-to-wallet transactions!');
  log('Press Ctrl+C to stop.');
}

main().catch(console.error);
