#!/usr/bin/env npx ts-node
/**
 * Rinku Proof Size Benchmark
 * 
 * Measures ACTUAL proof sizes from node-generated URLs:
 * - Profile A: Locally encoded (no node endpoint exists)
 * - Profile B: Uses proofUrl from /api/tx/{hash}/proof
 * - Profile C: Uses proofUrl from /api/txp/{hash}
 * 
 * Usage:
 *   npx ts-node scripts/proof-size-benchmark.ts [NODE_URL] [--samples N]
 */

import { deflate } from 'pako';

const NODE_URL = process.argv[2] || 'http://localhost:3001';

const samplesArg = process.argv.indexOf('--samples');
const SAMPLE_COUNT = samplesArg !== -1 ? parseInt(process.argv[samplesArg + 1]) : 20;

function base64urlEncode(data: Uint8Array): string {
  const base64 = Buffer.from(data).toString('base64');
  return base64.replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function base64urlDecode(str: string): Uint8Array {
  let base64 = str.replace(/-/g, '+').replace(/_/g, '/');
  while (base64.length % 4) base64 += '=';
  return new Uint8Array(Buffer.from(base64, 'base64'));
}

async function fetchJSON(path: string): Promise<any> {
  const res = await fetch(`${NODE_URL}${path}`);
  if (!res.ok) throw new Error(`HTTP ${res.status}: ${path}`);
  return res.json();
}

function getUrlPayloadSize(url: string): number {
  const match = url.match(/rinku:\/\/\w+\/(.+)/);
  if (!match) return 0;
  return base64urlDecode(match[1]).length;
}

function stats(arr: number[]): { min: number; max: number; avg: number; med: number } {
  if (arr.length === 0) return { min: 0, max: 0, avg: 0, med: 0 };
  const sorted = [...arr].sort((a, b) => a - b);
  return {
    min: sorted[0],
    max: sorted[sorted.length - 1],
    avg: sorted.reduce((a, b) => a + b, 0) / sorted.length,
    med: sorted[Math.floor(sorted.length / 2)],
  };
}

function fmt(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : `${(bytes / 1024).toFixed(1)} KB`;
}

function logSection(title: string) {
  console.log('\n' + '='.repeat(65));
  console.log(`  ${title}`);
  console.log('='.repeat(65));
}

async function main() {
  console.log('\n');
  console.log('╔═════════════════════════════════════════════════════════════════╗');
  console.log('║           RINKU PROOF SIZE BENCHMARK                            ║');
  console.log('║           (Node-generated proof URLs)                           ║');
  console.log('╚═════════════════════════════════════════════════════════════════╝');
  console.log(`\n  Node: ${NODE_URL}`);
  console.log(`  Samples: ${SAMPLE_COUNT}`);
  
  try {
    logSection('FETCHING DATA');
    
    const txData = await fetchJSON('/api/sync/transactions');
    const transactions = txData.transactions || [];
    console.log(`  Transactions: ${transactions.length}`);
    
    const finalizedTxs = transactions.filter((t: any) => t.finality);
    console.log(`  Finalized: ${finalizedTxs.length}`);
    
    const samples = finalizedTxs.length > 0 
      ? finalizedTxs.slice(0, SAMPLE_COUNT) 
      : transactions.slice(0, SAMPLE_COUNT);
    console.log(`  Using: ${samples.length} samples`);
    
    logSection('PROFILE A (Authorization - locally encoded)');
    
    const aSizes: number[] = [];
    const aUrls: number[] = [];
    
    for (const tx of samples) {
      const bundle = {
        tx: {
          from: tx.tx.from,
          to: tx.tx.to,
          amount: tx.tx.amount,
          fee: tx.tx.fee || 0,
          nonce: tx.tx.nonce,
          tipUrls: tx.tx.tipUrls || [],
          ts: tx.tx.ts,
          sig: tx.tx.sig,
        },
        fromPubKey: tx.publicKey ? Buffer.from(new Uint8Array(tx.publicKey)).toString('base64') : '',
        hash: tx.tx.hash,
      };
      
      const json = JSON.stringify(bundle);
      const compressed = deflate(new TextEncoder().encode(json));
      const payload = base64urlEncode(compressed);
      
      aSizes.push(compressed.length);
      aUrls.push(`rinku://tx/${payload}`.length);
    }
    
    const aStats = stats(aSizes);
    console.log(`  Samples: ${aSizes.length}`);
    console.log(`  Size: ${fmt(aStats.min)} - ${fmt(aStats.max)} (avg: ${fmt(aStats.avg)})`);
    console.log(`  URL: ${stats(aUrls).min} - ${stats(aUrls).max} chars`);
    
    logSection('PROFILE B (Node proofUrl from /api/tx/{hash}/proof)');
    
    const bSizes: number[] = [];
    const bUrls: number[] = [];
    let bSkip = 0;
    
    for (const tx of samples) {
      try {
        const proof = await fetchJSON(`/api/tx/${tx.tx.hash}/proof`);
        if (proof.proofUrl) {
          const size = getUrlPayloadSize(proof.proofUrl);
          bSizes.push(size);
          bUrls.push(proof.proofUrl.length);
        } else {
          bSkip++;
        }
      } catch {
        bSkip++;
      }
    }
    
    if (bSizes.length > 0) {
      const bStats = stats(bSizes);
      console.log(`  Samples: ${bSizes.length} (${bSkip} skipped)`);
      console.log(`  Size: ${fmt(bStats.min)} - ${fmt(bStats.max)} (avg: ${fmt(bStats.avg)})`);
      console.log(`  URL: ${stats(bUrls).min} - ${stats(bUrls).max} chars`);
    } else {
      console.log(`  \x1b[33mNo Profile B proofs available\x1b[0m`);
    }
    
    logSection('PROFILE C (Node proofUrl from /api/txp/{hash})');
    
    const cSizes: number[] = [];
    const cUrls: number[] = [];
    let cSkip = 0;
    
    for (const tx of samples) {
      try {
        const proof = await fetchJSON(`/api/txp/${tx.tx.hash}`);
        if (proof.proofUrl) {
          const size = getUrlPayloadSize(proof.proofUrl);
          cSizes.push(size);
          cUrls.push(proof.proofUrl.length);
        } else {
          cSkip++;
        }
      } catch {
        cSkip++;
      }
    }
    
    if (cSizes.length > 0) {
      const cStats = stats(cSizes);
      console.log(`  Samples: ${cSizes.length} (${cSkip} skipped)`);
      console.log(`  Size: ${fmt(cStats.min)} - ${fmt(cStats.max)} (avg: ${fmt(cStats.avg)})`);
      console.log(`  URL: ${stats(cUrls).min} - ${stats(cUrls).max} chars`);
    } else {
      console.log(`  \x1b[33mNo Profile C proofs available\x1b[0m`);
    }
    
    logSection('QR COMPATIBILITY');
    
    const qrL = 2953, qrH = 1273;
    
    console.log(`\n  Limits: QR-L=${qrL}B, QR-H=${qrH}B`);
    console.log('\n  ┌───────────┬──────────┬──────────┬──────────┬─────────┐');
    console.log('  │ Profile   │ Avg Size │ QR-L Fit │ QR-H Fit │ Count   │');
    console.log('  ├───────────┼──────────┼──────────┼──────────┼─────────┤');
    
    if (aSizes.length > 0) {
      const l = aSizes.filter(s => s <= qrL).length;
      const h = aSizes.filter(s => s <= qrH).length;
      console.log(`  │ Profile A │ ${fmt(stats(aSizes).avg).padEnd(8)} │ ${((l/aSizes.length)*100).toFixed(0).padStart(5)}%   │ ${((h/aSizes.length)*100).toFixed(0).padStart(5)}%   │ ${String(aSizes.length).padStart(7)} │`);
    }
    
    if (bSizes.length > 0) {
      const l = bSizes.filter(s => s <= qrL).length;
      const h = bSizes.filter(s => s <= qrH).length;
      console.log(`  │ Profile B │ ${fmt(stats(bSizes).avg).padEnd(8)} │ ${((l/bSizes.length)*100).toFixed(0).padStart(5)}%   │ ${((h/bSizes.length)*100).toFixed(0).padStart(5)}%   │ ${String(bSizes.length).padStart(7)} │`);
    }
    
    if (cSizes.length > 0) {
      const l = cSizes.filter(s => s <= qrL).length;
      const h = cSizes.filter(s => s <= qrH).length;
      console.log(`  │ Profile C │ ${fmt(stats(cSizes).avg).padEnd(8)} │ ${((l/cSizes.length)*100).toFixed(0).padStart(5)}%   │ ${((h/cSizes.length)*100).toFixed(0).padStart(5)}%   │ ${String(cSizes.length).padStart(7)} │`);
    }
    
    console.log('  └───────────┴──────────┴──────────┴──────────┴─────────┘');
    
    if (bSizes.length === 0 && cSizes.length === 0) {
      console.log('\n  \x1b[33mNote: B/C need finalized txs with checkpoints\x1b[0m');
    }
    
    console.log('\n' + '='.repeat(65));
    console.log('\x1b[32m  BENCHMARK COMPLETE\x1b[0m');
    console.log('='.repeat(65) + '\n');
    
  } catch (e: any) {
    console.error(`\n\x1b[31m  ERROR: ${e.message}\x1b[0m\n`);
    process.exit(1);
  }
}

main();
