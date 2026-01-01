import { parseTransactionURL, hashTransaction, type SignedTransaction } from '@rinku/core';

const NODE_URL = process.env.RINKU_NODE_URL || 'http://localhost:3001';
const FAUCET_URL = process.env.RINKU_FAUCET_URL || 'http://localhost:3002';

const WALLET_COUNT = parseInt(process.env.WALLET_COUNT || '20');

async function getTipUrls(): Promise<string[]> {
  const res = await fetch(`${NODE_URL}/api/tipUrls`);
  const data = await res.json();
  return data.tipUrls || [];
}

async function fundWallet(fingerprint: string): Promise<{ ok: boolean; txHash?: string }> {
  const res = await fetch(`${FAUCET_URL}/api/request`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ address: fingerprint })
  });
  if (res.ok) {
    const data = await res.json();
    return { ok: true, txHash: data.txHash };
  }
  return { ok: false };
}

async function sleep(ms: number) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

async function main() {
  console.log('='.repeat(60));
  console.log('RINKU NETWORK SIMULATION');
  console.log('='.repeat(60));
  console.log(`\nConfiguration:`);
  console.log(`  Wallets to create: ${WALLET_COUNT}`);
  console.log(`  Node: ${NODE_URL}`);
  console.log(`  Faucet: ${FAUCET_URL}\n`);

  const wallets: string[] = [];
  for (let i = 0; i < WALLET_COUNT; i++) {
    wallets.push(`wallet_${i.toString().padStart(3, '0')}_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 8)}`);
  }

  console.log('Creating transaction web via faucet...');
  console.log('-'.repeat(40));
  
  let funded = 0;
  let errors = 0;
  const startTime = Date.now();
  const txHashes: string[] = [];

  for (const wallet of wallets) {
    const result = await fundWallet(wallet);
    if (result.ok) {
      funded++;
      if (result.txHash) txHashes.push(result.txHash);
      process.stdout.write(`\r  Transactions: ${funded}/${WALLET_COUNT}`);
    } else {
      errors++;
    }
    await sleep(50);
  }
  
  const elapsed = ((Date.now() - startTime) / 1000).toFixed(2);
  console.log(`\n\n  Completed in ${elapsed}s`);
  console.log(`  TPS: ${(funded / parseFloat(elapsed)).toFixed(2)}`);
  console.log(`  Errors: ${errors}`);

  console.log('\n' + '='.repeat(60));
  console.log('VALIDATION FROM SINGLE TIP URL');
  console.log('='.repeat(60));

  const tipUrls = await getTipUrls();
  console.log(`\nCurrent tips: ${tipUrls.length}`);
  
  if (tipUrls.length > 0) {
    const startUrl = tipUrls[0];
    console.log(`\nStarting validation from: ${startUrl.slice(0, 50)}...`);
    
    const visited = new Set<string>();
    const queue = [startUrl];
    const transactions: SignedTransaction[] = [];
    let crawlErrors = 0;
    let maxDepth = 0;
    const depthMap = new Map<string, number>();
    depthMap.set(startUrl, 0);

    console.log('\nCrawling transaction graph...');
    const crawlStart = Date.now();
    
    while (queue.length > 0) {
      const url = queue.shift()!;
      if (visited.has(url)) continue;
      visited.add(url);

      const depth = depthMap.get(url) || 0;
      maxDepth = Math.max(maxDepth, depth);

      const tx = parseTransactionURL(url);
      if (!tx) {
        crawlErrors++;
        continue;
      }

      const signedTx: SignedTransaction = {
        ...tx,
        hash: hashTransaction(tx)
      };
      transactions.push(signedTx);

      for (const parentUrl of tx.tipUrls) {
        if (!visited.has(parentUrl)) {
          queue.push(parentUrl);
          depthMap.set(parentUrl, depth + 1);
        }
      }

      if (transactions.length % 5 === 0) {
        process.stdout.write(`\r  Crawled: ${transactions.length} transactions, depth: ${depth}, queue: ${queue.length}  `);
      }
    }

    const crawlElapsed = ((Date.now() - crawlStart) / 1000).toFixed(2);
    console.log(`\r  Crawled: ${transactions.length} transactions in ${crawlElapsed}s          `);

    console.log('\n' + '-'.repeat(40));
    console.log('CRAWL RESULTS:');
    console.log('-'.repeat(40));
    console.log(`  URLs visited: ${visited.size}`);
    console.log(`  Transactions found: ${transactions.length}`);
    console.log(`  Max depth (chain length): ${maxDepth}`);
    console.log(`  Crawl errors: ${crawlErrors}`);

    const accounts = new Map<string, number>();
    for (const tx of transactions) {
      if (tx.from !== 'genesis' && tx.from !== 'faucet') {
        accounts.set(tx.from, (accounts.get(tx.from) || 0) - tx.amount);
      }
      accounts.set(tx.to, (accounts.get(tx.to) || 0) + tx.amount);
    }

    console.log(`  Unique accounts: ${accounts.size}`);

    const sampleAccounts = Array.from(accounts.entries())
      .filter(([fp]) => fp.startsWith('wallet_'))
      .slice(0, 5);
    
    if (sampleAccounts.length > 0) {
      console.log('\n  Sample wallet balances (from crawl):');
      for (const [fp, balance] of sampleAccounts) {
        console.log(`    ${fp.slice(0, 25)}... : ${balance} coins`);
      }
    }

    console.log('\n' + '-'.repeat(40));
    console.log('URL LINKING STRUCTURE:');
    console.log('-'.repeat(40));

    const sampleTxs = transactions.slice(-5);
    for (const tx of sampleTxs) {
      const shortHash = typeof tx.hash === 'string' ? tx.hash.slice(0, 8) : 'n/a';
      const parents = tx.tipUrls.map(u => {
        const p = parseTransactionURL(u);
        return p ? `${p.from.slice(0,8)}→${p.to.slice(0,8)}` : 'genesis';
      }).join(', ');
      const fromStr = tx.from.slice(0, 12);
      const toStr = tx.to.slice(0, 12);
      console.log(`  ${shortHash}: ${fromStr}→${toStr} refs [${parents || 'genesis'}]`);
    }

    console.log('\n' + '='.repeat(60));
    console.log('THE LEDGER IS SELF-CRAWLABLE!');
    console.log('='.repeat(60));
    console.log(`\nFrom a single tip URL, we reconstructed:`);
    console.log(`  - ${transactions.length} transactions`);
    console.log(`  - ${accounts.size} account balances`);
    console.log(`  - ${maxDepth} levels deep transaction chain`);
    console.log(`  - Complete transaction graph via tipUrls\n`);
    console.log(`No node infrastructure needed - just the URLs themselves.`);
    console.log(`Anyone can validate the entire ledger from any tip URL.\n`);
  }

  const dagRes = await fetch(`${NODE_URL}/api/dag`);
  const dag = await dagRes.json();
  console.log('NODE STATE VERIFICATION:');
  console.log('-'.repeat(40));
  console.log(`  Total transactions in DAG: ${dag.nodes?.length || dag.transactions?.length || 0}`);
  console.log(`  Current tips: ${dag.tips?.length || 0}`);
  
  const accountsRes = await fetch(`${NODE_URL}/api/accounts`);
  const accountsData = await accountsRes.json();
  console.log(`  Accounts tracked: ${accountsData.accounts?.length || 0}`);
  console.log();
}

main().catch(console.error);
