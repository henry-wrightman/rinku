#!/usr/bin/env tsx
/**
 * Test proof integrity by creating transactions and verifying their proofs
 */
import { Wallet } from "@rinku/wallet";
import { sign, hashTransaction } from "@rinku/core";

const NODE_URL = process.env.RINKU_NODE_URL || "http://localhost:3001";
const FAUCET_URL = process.env.RINKU_FAUCET_URL || "http://localhost:3002";

async function getTips(): Promise<string[]> {
  const res = await fetch(`${NODE_URL}/api/dag/summary`);
  if (!res.ok) return [];
  const data = await res.json();
  return data.tips || [];
}

async function testProofIntegrity() {
  console.log('=== PROOF INTEGRITY TEST ===\n');
  
  // 1. Create wallet and fund it
  const wallet = new Wallet(NODE_URL);
  await wallet.create();
  const address = wallet.getFingerprint();
  const keyPair = (wallet as any).keyManager.getKeyPair();
  console.log(`Created wallet: ${address}`);
  
  // Fund wallet
  const fundRes = await fetch(`${FAUCET_URL}/api/request`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ address })
  });
  if (!fundRes.ok) {
    const err = await fundRes.text();
    throw new Error(`Failed to fund wallet: ${err}`);
  }
  const fundData = await fundRes.json();
  console.log(`Funded wallet with ${fundData.amount} RKU`);
  
  // Wait for checkpoint
  console.log('Waiting for checkpoint...');
  await new Promise(r => setTimeout(r, 4000));
  
  // Refresh wallet state
  const state = await wallet.refresh();
  let nonce = state.nonce;
  console.log(`Wallet nonce: ${nonce}, balance: ${state.balance}`);
  
  // 2. Create test transactions
  const recipient = new Wallet(NODE_URL);
  await recipient.create();
  const recipientAddr = recipient.getFingerprint();
  console.log(`\nRecipient: ${recipientAddr}`);
  
  const txHashes: string[] = [];
  
  for (let i = 0; i < 5; i++) {
    try {
      const tips = await getTips();
      const ts = Date.now();
      const amount = 0.5;
      const fee = 0.01;
      
      // Build transaction for hashing (same format as mega-stress)
      const txForHash = {
        from: address,
        to: recipientAddr,
        amount,
        fee,
        nonce: nonce,
        tipUrls: tips.slice(0, 2),
        sig: "",
        ts,
      };
      
      const hash = await hashTransaction(txForHash);
      const sig = await sign(hash, keyPair.privateKey);
      
      const tx = {
        from: address,
        to: recipientAddr,
        amount,
        nonce: nonce,
        ts,
        parents: tips.slice(0, 2),
        fee,
        sig,
        hash,
      };
      
      nonce++;
      
      const submitRes = await fetch(`${NODE_URL}/api/transaction`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(tx)
      });
      
      if (submitRes.ok) {
        console.log(`Submitted tx ${i + 1}: ${tx.hash.slice(0, 16)}...`);
        txHashes.push(tx.hash);
      } else {
        const err = await submitRes.text();
        console.log(`Failed tx ${i + 1}: ${err}`);
      }
    } catch (e: any) {
      console.log(`Error creating tx ${i + 1}: ${e.message}`);
    }
  }
  
  // Wait for finalization
  console.log('\nWaiting for finalization...');
  await new Promise(r => setTimeout(r, 5000));
  
  // 3. Test proof retrieval and verification for each transaction
  console.log('\n=== VERIFYING PROOFS ===\n');
  
  let validProofs = 0;
  let invalidProofs = 0;
  let notFinalized = 0;
  let errors = 0;
  
  for (const hash of txHashes) {
    try {
      // Get transaction details
      const txRes = await fetch(`${NODE_URL}/api/transaction/${hash}`);
      if (!txRes.ok) {
        console.log(`[${hash.slice(0,8)}] Not found`);
        notFinalized++;
        continue;
      }
      const txData = await txRes.json();
      
      if (!txData.finalized) {
        console.log(`[${hash.slice(0,8)}] Not finalized yet`);
        notFinalized++;
        continue;
      }
      
      // Get proof
      const proofRes = await fetch(`${NODE_URL}/api/transaction/${hash}/proof`);
      if (!proofRes.ok) {
        console.log(`[${hash.slice(0,8)}] Proof endpoint failed: ${await proofRes.text()}`);
        errors++;
        continue;
      }
      const proofData = await proofRes.json();
      
      if (!proofData.proof) {
        console.log(`[${hash.slice(0,8)}] No proof data`);
        errors++;
        continue;
      }
      
      // Verify proof using the verify endpoint
      const verifyRes = await fetch(`${NODE_URL}/api/verify`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ proof: proofData.proof })
      });
      
      if (!verifyRes.ok) {
        console.log(`[${hash.slice(0,8)}] Verify endpoint failed: ${await verifyRes.text()}`);
        invalidProofs++;
        continue;
      }
      
      const verifyData = await verifyRes.json();
      if (verifyData.valid) {
        console.log(`[${hash.slice(0,8)}] Valid proof`);
        validProofs++;
      } else {
        console.log(`[${hash.slice(0,8)}] Invalid proof: ${verifyData.error || 'unknown'}`);
        invalidProofs++;
      }
    } catch (e: any) {
      console.log(`[${hash.slice(0,8)}] Error: ${e.message}`);
      errors++;
    }
  }
  
  // 4. Summary
  console.log('\n=== SUMMARY ===');
  console.log(`Total transactions: ${txHashes.length}`);
  console.log(`Valid proofs: ${validProofs}`);
  console.log(`Invalid proofs: ${invalidProofs}`);
  console.log(`Not finalized: ${notFinalized}`);
  console.log(`Errors: ${errors}`);
  
  if (invalidProofs > 0) {
    console.log('\nINTEGRITY ISSUES DETECTED - Some proofs are invalid!');
    process.exit(1);
  } else if (validProofs === txHashes.length) {
    console.log('\nAll proofs verified successfully!');
  } else {
    console.log('\nSome transactions not finalized yet - run again after more time');
  }
}

testProofIntegrity().catch(e => {
  console.error('Test failed:', e);
  process.exit(1);
});
