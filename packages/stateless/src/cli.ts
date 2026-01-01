#!/usr/bin/env node

import { StatelessValidator, extractUrlsFromText } from './index.js';

async function main() {
  const args = process.argv.slice(2);
  
  if (args.length === 0) {
    console.log(`
rinku stateless validator
=========================

validate a ledger from transaction URLs alone - no node required.

usage:
  npx @rinku/stateless <url1> [url2] [url3] ...
  
  or pipe URLs via stdin:
  echo "/tx/eJy..." | npx @rinku/stateless --stdin

examples:
  # validate from genesis URL
  npx @rinku/stateless "/tx/eJyLzkjNyclX..."

  # validate from multiple tip URLs  
  npx @rinku/stateless "/tx/abc..." "/tx/def..."

  # validate from remote node
  npx @rinku/stateless "https://node.example.com/tx/..."

  # extract URLs from a file and validate
  cat ledger-dump.txt | npx @rinku/stateless --stdin

the validator will:
1. decode each transaction URL
2. crawl parent transactions via 'tips' field
3. reconstruct the complete DAG
4. verify all balances and transfers
5. report any validation errors
`);
    return;
  }

  let urls: string[] = [];

  if (args.includes('--stdin')) {
    const chunks: Buffer[] = [];
    for await (const chunk of process.stdin) {
      chunks.push(chunk);
    }
    const input = Buffer.concat(chunks).toString('utf-8');
    urls = extractUrlsFromText(input);
  } else {
    urls = args.filter(arg => !arg.startsWith('--'));
  }

  if (urls.length === 0) {
    console.error('error: no transaction URLs provided');
    process.exit(1);
  }

  console.log(`\nvalidating ${urls.length} transaction URL(s)...\n`);

  const validator = new StatelessValidator();
  const result = await validator.validateFromUrls(urls, {
    maxDepth: 1000,
    timeout: 10000
  });

  console.log('=== validation result ===\n');
  console.log(`status: ${result.valid ? 'VALID' : 'INVALID'}`);
  console.log(`transactions: ${result.stats.totalTransactions}`);
  console.log(`accounts: ${result.stats.totalAccounts}`);
  console.log(`urls crawled: ${result.stats.crawledUrls}`);
  console.log(`failed urls: ${result.stats.failedUrls}`);
  console.log(`tips: ${result.tips.length}`);

  if (result.errors.length > 0) {
    console.log('\n=== errors ===\n');
    for (const error of result.errors) {
      console.log(`  - ${error}`);
    }
  }

  console.log('\n=== accounts ===\n');
  const sortedAccounts = Array.from(result.accounts.entries())
    .sort((a, b) => b[1].balance - a[1].balance);
  
  for (const [fingerprint, account] of sortedAccounts.slice(0, 10)) {
    console.log(`  ${fingerprint.slice(0, 12)}... : ${account.balance} coins`);
  }

  if (sortedAccounts.length > 10) {
    console.log(`  ... and ${sortedAccounts.length - 10} more accounts`);
  }

  console.log('\n=== tips (current ledger heads) ===\n');
  for (const tip of result.tips.slice(0, 5)) {
    const tx = result.transactions.get(tip);
    if (tx) {
      console.log(`  ${tip.slice(0, 12)}... : ${tx.from.slice(0, 8)}→${tx.to.slice(0, 8)} (${tx.amount})`);
    }
  }

  if (result.tips.length > 5) {
    console.log(`  ... and ${result.tips.length - 5} more tips`);
  }

  process.exit(result.valid ? 0 : 1);
}

main().catch(err => {
  console.error('fatal error:', err.message);
  process.exit(1);
});
