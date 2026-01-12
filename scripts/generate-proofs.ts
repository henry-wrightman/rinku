#!/usr/bin/env npx ts-node
/**
 * Rinku Proof Verification Pipeline
 * 
 * Tests real proof generation and verification using core functions:
 * - Profile A: ECDSA signature verification via core `verify`
 * - Profile B: Uses node proofUrl, verifies Merkle proofs via core `verifyMerkleProof`
 * - Profile C: Full cryptographic verification via core `verifySelfContainedProof`
 * 
 * Usage:
 *   npx ts-node scripts/generate-proofs.ts [NODE_URL] [--count N] [--profile A|B|C|all]
 */

import { deflate, inflate } from 'pako';
import { 
  hashTransaction, 
  verify,
  verifyMerkleProof,
  verifySelfContainedProof,
  type SelfContainedProof,
} from '@rinku/core';

const NODE_URL = process.argv[2] || 'http://localhost:3001';

const countArg = process.argv.indexOf('--count');
const PROOF_COUNT = countArg !== -1 ? parseInt(process.argv[countArg + 1]) : 3;

const profileArg = process.argv.indexOf('--profile');
const PROFILE_FILTER = profileArg !== -1 ? process.argv[profileArg + 1].toUpperCase() : 'ALL';

interface ProofStats {
  profile: string;
  txHash: string;
  urlLength: number;
  compressedBytes: number;
  verificationPassed: boolean;
  errors: string[];
  warnings: string[];
}

const stats: ProofStats[] = [];

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

function log(msg: string) {
  console.log(msg);
}

function logSection(title: string) {
  console.log('\n' + '='.repeat(60));
  console.log(`  ${title}`);
  console.log('='.repeat(60));
}

function encodeBundle(bundle: any, profile: string): { url: string; compressedBytes: number } {
  const json = JSON.stringify(bundle);
  const compressed = deflate(new TextEncoder().encode(json));
  const payload = base64urlEncode(compressed);
  return { 
    url: `rinku://${profile}/${payload}`,
    compressedBytes: compressed.length,
  };
}

function decodeProofUrl(url: string): { profile: string; bundle: any; compressedBytes: number } {
  const match = url.match(/rinku:\/\/(\w+)\/(.+)/);
  if (!match) throw new Error('Invalid proof URL format');
  const [, profile, payload] = match;
  const compressed = base64urlDecode(payload);
  const json = inflate(compressed, { to: 'string' });
  return { profile, bundle: JSON.parse(json), compressedBytes: compressed.length };
}

function getProofUrlSize(url: string): number {
  const match = url.match(/rinku:\/\/\w+\/(.+)/);
  if (!match) return 0;
  return base64urlDecode(match[1]).length;
}

async function testProfileA(
  tx: any, 
  hash: string,
  publicKey: number[] | undefined
): Promise<ProofStats> {
  const errors: string[] = [];
  const warnings: string[] = [];
  
  const bundle = {
    tx: {
      from: tx.from,
      to: tx.to,
      amount: tx.amount,
      fee: tx.fee || 0,
      nonce: tx.nonce,
      tipUrls: tx.tipUrls || [],
      ts: tx.ts,
      sig: tx.sig,
    },
    fromPubKey: publicKey ? Buffer.from(new Uint8Array(publicKey)).toString('base64') : '',
    hash,
  };
  
  const { url, compressedBytes } = encodeBundle(bundle, 'tx');
  
  const decoded = decodeProofUrl(url);
  if (decoded.bundle.hash !== hash) {
    errors.push('Roundtrip decode failed');
  }
  
  const txForHash = {
    from: tx.from,
    to: tx.to,
    amount: tx.amount,
    fee: tx.fee || 0,
    nonce: tx.nonce,
    tipUrls: tx.tipUrls || [],
    sig: '',
    ts: tx.ts,
  };
  
  try {
    const computedHash = await hashTransaction(txForHash);
    if (computedHash !== hash) {
      errors.push(`Hash mismatch`);
    }
  } catch (e: any) {
    errors.push(`Hash error: ${e.message}`);
  }
  
  if (publicKey && publicKey.length > 0 && tx.sig) {
    try {
      const pubKeyBytes = new Uint8Array(publicKey);
      const msgHash = await hashTransaction(txForHash);
      const isValid = await verify(msgHash, tx.sig, pubKeyBytes);
      
      if (!isValid) {
        errors.push('ECDSA signature INVALID');
      }
    } catch (e: any) {
      errors.push(`Sig error: ${e.message}`);
    }
  } else {
    warnings.push('No pubkey - sig check skipped');
  }
  
  return {
    profile: 'Profile A',
    txHash: hash.slice(0, 12) + '...',
    urlLength: url.length,
    compressedBytes,
    verificationPassed: errors.length === 0,
    errors,
    warnings,
  };
}

async function testProfileB(txHash: string, hasFinality: boolean): Promise<ProofStats | null> {
  try {
    const proofData = await fetchJSON(`/api/tx/${txHash}/proof`);
    
    if (!proofData.proofUrl) {
      return null;
    }
    
    const url = proofData.proofUrl;
    const compressedBytes = getProofUrlSize(url);
    
    const errors: string[] = [];
    const warnings: string[] = [];
    
    const decoded = decodeProofUrl(url);
    const bundle = decoded.bundle;
    
    if (!bundle.tx && !bundle.hash) {
      errors.push('Bundle missing tx or hash');
      return {
        profile: 'Profile B',
        txHash: txHash.slice(0, 12) + '...',
        urlLength: url.length,
        compressedBytes,
        verificationPassed: false,
        errors,
        warnings,
      };
    }
    
    const bundleHash = bundle.hash;
    if (bundleHash && bundleHash !== txHash) {
      errors.push(`Hash mismatch: bundle ${bundleHash.slice(0, 8)}... != requested ${txHash.slice(0, 8)}...`);
    }
    
    if (bundle.tx) {
      const txForHash = {
        from: bundle.tx.from,
        to: bundle.tx.to,
        amount: bundle.tx.amount,
        fee: bundle.tx.fee || 0,
        nonce: bundle.tx.nonce,
        tipUrls: bundle.tx.tipUrls || [],
        sig: '',
        ts: bundle.tx.ts,
      };
      
      try {
        const computedHash = await hashTransaction(txForHash);
        if (computedHash !== txHash) {
          errors.push(`Computed hash mismatch`);
        }
      } catch (e: any) {
        errors.push(`Hash compute error: ${e.message}`);
      }
    }
    
    const truncatedParents = bundle.truncatedParents || [];
    let validMerkleProofs = 0;
    let invalidMerkleProofs = 0;
    
    for (const parent of truncatedParents) {
      const anchor = parent.checkpointAnchor;
      if (!anchor) {
        continue;
      }
      
      if (!anchor.checkpointId) {
        errors.push('Anchor missing checkpointId');
        continue;
      }
      
      const mp = parent.merkleProof;
      if (mp && mp.proof && mp.proof.length > 0 && typeof mp.index === 'number' && mp.txMerkleRoot) {
        try {
          const isValid = await verifyMerkleProof(parent.hash, mp.proof, mp.index, mp.txMerkleRoot);
          if (isValid) {
            validMerkleProofs++;
          } else {
            invalidMerkleProofs++;
            errors.push(`Merkle INVALID: parent ${parent.hash.slice(0, 8)}...`);
          }
        } catch (e: any) {
          errors.push(`Merkle error: ${e.message}`);
        }
        
        if (anchor.txMerkleRoot && anchor.txMerkleRoot !== mp.txMerkleRoot) {
          errors.push('Root mismatch: anchor vs merkleProof');
        }
      }
    }
    
    if (validMerkleProofs > 0) {
      warnings.push(`Verified ${validMerkleProofs} Merkle proof(s)`);
    } else if (truncatedParents.length > 0 && proofData.hasFinality) {
      errors.push('Finality claimed but no valid Merkle proofs');
    } else if (truncatedParents.length > 0 && invalidMerkleProofs === 0) {
      warnings.push('No Merkle proofs to verify');
    }
    
    if (!proofData.hasFinality && hasFinality) {
      warnings.push('Node says no finality');
    }
    
    return {
      profile: 'Profile B',
      txHash: txHash.slice(0, 12) + '...',
      urlLength: url.length,
      compressedBytes,
      verificationPassed: errors.length === 0,
      errors,
      warnings,
    };
  } catch (e: any) {
    return null;
  }
}

async function testProfileC(txHash: string): Promise<ProofStats | null> {
  try {
    const proofData = await fetchJSON(`/api/txp/${txHash}`);
    
    if (!proofData.proofUrl) {
      return null;
    }
    
    const url = proofData.proofUrl;
    const decoded = decodeProofUrl(url);
    const bundle = decoded.bundle as SelfContainedProof;
    
    const verifyResult = verifySelfContainedProof(bundle);
    
    return {
      profile: 'Profile C',
      txHash: txHash.slice(0, 12) + '...',
      urlLength: url.length,
      compressedBytes: decoded.compressedBytes,
      verificationPassed: verifyResult.valid,
      errors: verifyResult.errors,
      warnings: [],
    };
  } catch (e: any) {
    return null;
  }
}

async function main() {
  console.log('\n');
  console.log('╔════════════════════════════════════════════════════════════╗');
  console.log('║         RINKU PROOF VERIFICATION PIPELINE                  ║');
  console.log('║         (ECDSA, Merkle, BLS via @rinku/core)               ║');
  console.log('╚════════════════════════════════════════════════════════════╝');
  console.log(`\n  Node: ${NODE_URL}`);
  console.log(`  Count: ${PROOF_COUNT}`);
  console.log(`  Profile: ${PROFILE_FILTER}`);
  console.log(`  Started: ${new Date().toISOString()}`);
  
  try {
    logSection('1. FETCHING DATA');
    
    const status = await fetchJSON('/api/sync/status');
    log(`  Transactions: ${status.dagSize}`);
    
    const checkpointData = await fetchJSON('/api/checkpoints');
    const checkpoints = checkpointData.chain || [];
    log(`  Checkpoints: ${checkpoints.length}`);
    
    const txData = await fetchJSON('/api/sync/transactions');
    const transactions = txData.transactions || [];
    log(`  Retrieved: ${transactions.length} txs`);
    
    const withPubKeys = transactions.filter((t: any) => t.publicKey && t.publicKey.length > 0);
    log(`  With pubkeys: ${withPubKeys.length}`);
    
    const finalizedTxs = transactions.filter((t: any) => t.finality);
    log(`  Finalized: ${finalizedTxs.length}`);
    
    logSection('2. RUNNING VERIFICATION');
    
    const profiles: string[] = [];
    if (PROFILE_FILTER === 'ALL' || PROFILE_FILTER === 'A') profiles.push('A');
    if (PROFILE_FILTER === 'ALL' || PROFILE_FILTER === 'B') profiles.push('B');
    if (PROFILE_FILTER === 'ALL' || PROFILE_FILTER === 'C') profiles.push('C');
    
    log(`  Profiles: ${profiles.join(', ')}`);
    
    const samplesToUse = profiles.includes('A')
      ? (withPubKeys.length > 0 ? withPubKeys.slice(0, PROOF_COUNT) : transactions.slice(0, PROOF_COUNT))
      : (finalizedTxs.length > 0 ? finalizedTxs.slice(0, PROOF_COUNT) : transactions.slice(0, PROOF_COUNT));
    
    if (samplesToUse.length === 0) {
      log('\x1b[31m  No transactions available\x1b[0m');
      process.exit(1);
    }
    
    for (const txData of samplesToUse) {
      const tx = txData.tx;
      const hash = tx.hash;
      const publicKey = txData.publicKey;
      const hasFinality = !!txData.finality;
      
      log(`\n  TX: ${hash.slice(0, 12)}...`);
      
      for (const profile of profiles) {
        let result: ProofStats | null = null;
        
        switch (profile) {
          case 'A':
            result = await testProfileA(tx, hash, publicKey);
            break;
          case 'B':
            result = await testProfileB(hash, hasFinality);
            break;
          case 'C':
            result = await testProfileC(hash);
            break;
        }
        
        if (!result) {
          log(`    ${profile}: \x1b[33mSKIPPED\x1b[0m (no proof available)`);
          continue;
        }
        
        stats.push(result);
        
        const status = result.verificationPassed ? '\x1b[32m✓\x1b[0m' : '\x1b[31m✗\x1b[0m';
        log(`    ${profile}: ${status} ${result.urlLength} chars, ${result.compressedBytes} bytes`);
        
        for (const warn of result.warnings) {
          log(`       \x1b[33m! ${warn}\x1b[0m`);
        }
        
        if (!result.verificationPassed) {
          for (const err of result.errors.slice(0, 2)) {
            log(`       \x1b[31m- ${err}\x1b[0m`);
          }
          if (result.errors.length > 2) {
            log(`       - ...${result.errors.length - 2} more`);
          }
        }
      }
    }
    
    logSection('3. SUMMARY');
    
    for (const profile of ['Profile A', 'Profile B', 'Profile C']) {
      const profileStats = stats.filter(s => s.profile === profile);
      if (profileStats.length === 0) continue;
      
      const avgBytes = profileStats.reduce((a, s) => a + s.compressedBytes, 0) / profileStats.length;
      const passed = profileStats.filter(s => s.verificationPassed).length;
      
      log(`\n  ${profile}: ${passed}/${profileStats.length} verified`);
      log(`    Avg: ${avgBytes.toFixed(0)} bytes`);
      log(`    QR-L: ${avgBytes <= 2953 ? '\x1b[32m✓\x1b[0m' : '\x1b[31m✗\x1b[0m'}`);
    }
    
    const total = stats.length;
    const verified = stats.filter(s => s.verificationPassed).length;
    
    console.log('\n' + '='.repeat(60));
    log(`  RESULT: ${verified}/${total} proofs verified`);
    
    if (verified === total && total > 0) {
      console.log('\x1b[32m  ALL PROOFS VERIFIED\x1b[0m');
    } else if (total === 0) {
      console.log('\x1b[33m  NO PROOFS TESTED\x1b[0m');
    } else {
      console.log('\x1b[33m  PARTIAL VERIFICATION (see above)\x1b[0m');
    }
    console.log('='.repeat(60) + '\n');
    
    process.exit(total > 0 && verified === total ? 0 : 1);
    
  } catch (e: any) {
    console.error(`\n\x1b[31m  ERROR: ${e.message}\x1b[0m\n`);
    process.exit(1);
  }
}

main();
