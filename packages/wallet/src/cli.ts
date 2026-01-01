import { Wallet } from './index.js';

const NODE_URL = process.env.RINKU_NODE_URL || 'http://localhost:3001';
const FAUCET_URL = process.env.RINKU_FAUCET_URL || 'http://localhost:3002';

async function main() {
  const args = process.argv.slice(2);
  const command = args[0];

  if (!command) {
    console.log(`
rinku wallet cli

commands:
  new                    create a new wallet
  import <key>           import wallet from exported key
  balance <address>      check balance of an address
  faucet <address>       request coins from faucet
  send <from-key> <to> <amount>  send coins
  tips                   show current DAG tips
  dag                    show full DAG state

environment variables:
  RINKU_NODE_URL         node api url (default: http://localhost:3001)
  RINKU_FAUCET_URL       faucet api url (default: http://localhost:3002)

examples:
  npx tsx src/cli.ts new
  npx tsx src/cli.ts faucet abc123...
  npx tsx src/cli.ts send "exported-key-json" def456... 50

  # connect to remote node:
  RINKU_NODE_URL=https://your-node.replit.dev RINKU_FAUCET_URL=https://your-node.replit.dev npx tsx src/cli.ts faucet abc123...
`);
    return;
  }

  if (command === 'new') {
    const wallet = new Wallet(NODE_URL);
    const fingerprint = await wallet.create();
    console.log('\nnew wallet created!\n');
    console.log('address (fingerprint):');
    console.log(fingerprint);
    console.log('\nprivate key (keep secret, use for import/send):');
    console.log(wallet.export());
    console.log('\nnext: request coins with');
    console.log(`npx tsx src/cli.ts faucet ${fingerprint}`);
    return;
  }

  if (command === 'import') {
    const key = args[1];
    if (!key) {
      console.log('usage: import <exported-key>');
      return;
    }
    const wallet = new Wallet(NODE_URL);
    const fingerprint = await wallet.import(key);
    const state = await wallet.refresh();
    console.log('\nwallet imported!\n');
    console.log('address:', fingerprint);
    console.log('balance:', state.balance);
    console.log('nonce:', state.nonce);
    return;
  }

  if (command === 'balance') {
    const address = args[1];
    if (!address) {
      console.log('usage: balance <address>');
      return;
    }
    try {
      const res = await fetch(`${NODE_URL}/api/account/${address}`);
      if (res.ok) {
        const data = await res.json() as { balance: number; nonce: number };
        console.log('\nbalance:', data.balance);
        console.log('nonce:', data.nonce);
      } else {
        console.log('\naccount not found (balance: 0)');
      }
    } catch {
      console.log('error: could not connect to node');
    }
    return;
  }

  if (command === 'faucet') {
    const address = args[1];
    if (!address) {
      console.log('usage: faucet <address>');
      return;
    }
    try {
      const res = await fetch(`${FAUCET_URL}/api/faucet/request`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ address })
      });
      const data = await res.json() as { success?: boolean; amount?: number; txHash?: string; error?: string };
      if (res.ok && data.success) {
        console.log('\nfaucet request successful!');
        console.log('received:', data.amount, 'coins');
        console.log('tx hash:', data.txHash);
      } else {
        console.log('\nerror:', data.error);
      }
    } catch {
      console.log('error: could not connect to faucet');
    }
    return;
  }

  if (command === 'send') {
    const exportedKey = args[1];
    const to = args[2];
    const amount = parseInt(args[3], 10);
    
    if (!exportedKey || !to || !amount) {
      console.log('usage: send <from-key> <to-address> <amount>');
      return;
    }

    try {
      const wallet = new Wallet(NODE_URL);
      await wallet.import(exportedKey);
      console.log('\nsending', amount, 'coins to', to.slice(0, 8) + '...');
      
      const { tx, url } = await wallet.send(to, amount);
      const pubkeyB64 = Buffer.from(wallet.getPublicKey()).toString('base64url');
      
      console.log('\ntransaction sent!');
      console.log('tx hash:', tx.hash);
      console.log('from:', tx.from.slice(0, 8) + '...');
      console.log('to:', tx.to.slice(0, 8) + '...');
      console.log('amount:', tx.amount);
      console.log('\ntransaction url (view):');
      console.log(NODE_URL + url);
      console.log('\ntransaction url (submit):');
      console.log(NODE_URL + url + '?submit=true&pubkey=' + pubkeyB64);
    } catch (err: any) {
      console.log('\nerror:', err.message);
    }
    return;
  }

  if (command === 'tips') {
    try {
      const res = await fetch(`${NODE_URL}/api/tipUrls`);
      const data = await res.json() as { tipUrls: string[] };
      console.log('\ncurrent tip URLs (the ledger heads):');
      data.tipUrls.forEach((url, i) => {
        console.log(`  ${i}: ${url.slice(0, 40)}...`);
      });
      console.log('\nthese URLs contain the full transaction data - shareable and self-validating');
    } catch {
      console.log('error: could not connect to node');
    }
    return;
  }

  if (command === 'dag') {
    try {
      const res = await fetch(`${NODE_URL}/api/dag`);
      const data = await res.json() as { nodes: any[]; tips: string[]; tipUrls: string[]; merkleRoot: string };
      console.log('\ndag state:');
      console.log('total nodes:', data.nodes.length);
      console.log('active tips:', data.tips.length);
      console.log('merkle root:', data.merkleRoot.slice(0, 16) + '...');
      console.log('\ntransactions:');
      data.nodes.forEach((node, i) => {
        console.log(`\n  [${i}] ${node.tx.hash.slice(0, 12)}...`);
        console.log(`      ${node.tx.from === 'genesis' ? 'genesis' : node.tx.from.slice(0, 8) + '...'} → ${node.tx.to.slice(0, 8)}...`);
        console.log(`      amount: ${node.tx.amount}, weight: ${node.weight}`);
        const parentUrls = node.parentUrls || node.parents || [];
        if (parentUrls.length > 0) {
          console.log(`      refs: ${parentUrls.length} parent URL(s)`);
        }
      });
      
      if (data.tipUrls && data.tipUrls.length > 0) {
        console.log('\n\ncurrent tip URLs (share these to replicate the ledger):');
        data.tipUrls.slice(0, 3).forEach((url: string) => {
          console.log(`  ${url.slice(0, 50)}...`);
        });
      }
    } catch {
      console.log('error: could not connect to node');
    }
    return;
  }

  console.log('unknown command:', command);
}

main().catch(console.error);
