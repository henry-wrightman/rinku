import {
  extractProofFromUrl,
  verifyCheckpointProof,
  decodeTransaction
} from '@rinku/core';

const NODE_URL = process.env.RINKU_NODE_URL || 'http://localhost:3001';

interface CheckpointCreateResponse {
  success: boolean;
  checkpoint?: { checkpointId: string };
}

interface DAGResponse {
  nodes: { tx: { hash: string; from: string; to: string; amount: number; ts: number } }[];
}

interface FinalizedResponse {
  finalized: boolean;
  txUrl?: string;
  finalizedUrl?: string;
  reason?: string;
  proof?: object;
}

async function main() {
  console.log('=== Rinku Finality Proof Demo ===\n');
  console.log('This demo shows how a single URL can prove transaction finality');
  console.log('without requiring any node infrastructure.\n');

  console.log('1. Creating a checkpoint on the network...');
  const createRes = await fetch(`${NODE_URL}/api/checkpoints/create`, {
    method: 'POST'
  });
  const createData = await createRes.json() as CheckpointCreateResponse;
  console.log(`   Checkpoint created: ${createData.checkpoint?.checkpointId || 'already exists'}\n`);

  console.log('2. Getting a transaction with finality proof...');
  const dagRes = await fetch(`${NODE_URL}/api/dag`);
  const dagData = await dagRes.json() as DAGResponse;
  
  if (!dagData.nodes || dagData.nodes.length === 0) {
    console.log('   No transactions found in the DAG');
    return;
  }

  const sampleTx = dagData.nodes[0];
  console.log(`   Transaction hash: ${sampleTx.tx.hash.slice(0, 16)}...`);
  console.log(`   Amount: ${sampleTx.tx.amount} coins`);
  console.log(`   From: ${sampleTx.tx.from.slice(0, 12)}... -> To: ${sampleTx.tx.to.slice(0, 12)}...\n`);

  console.log('3. Getting finalized URL with embedded proof...');
  const finalizedRes = await fetch(`${NODE_URL}/api/tx/${sampleTx.tx.hash}/finalized`);
  const finalizedData = await finalizedRes.json() as FinalizedResponse;

  if (!finalizedData.finalized) {
    console.log(`   Transaction not yet finalized: ${finalizedData.reason}`);
    return;
  }

  console.log(`   Original URL length: ${finalizedData.txUrl!.length} chars`);
  console.log(`   Finalized URL length: ${finalizedData.finalizedUrl!.length} chars`);
  console.log(`   Proof adds: ${finalizedData.finalizedUrl!.length - finalizedData.txUrl!.length} chars\n`);

  console.log('4. Extracting and verifying proof from URL alone...');
  console.log('   (This is what a recipient would do - NO node access needed)\n');

  const { txUrl, proof } = extractProofFromUrl(finalizedData.finalizedUrl!);
  
  if (!proof) {
    console.log('   ERROR: Could not extract proof from URL');
    return;
  }

  console.log(`   Extracted proof:`);
  console.log(`   - Checkpoint ID: ${proof.checkpointId}`);
  console.log(`   - Checkpoint Height: ${proof.checkpointHeight}`);
  console.log(`   - Merkle Root: ${proof.merkleRoot.slice(0, 16)}...`);
  console.log(`   - Signature Count: ${proof.signatureCount}`);
  console.log(`   - Validator Weight: ${proof.totalValidatorWeight.toFixed(1)}%\n`);

  console.log('5. Verifying checkpoint proof...');
  const verification = await verifyCheckpointProof(proof);
  
  console.log(`   Valid: ${verification.valid}`);
  console.log(`   Signature Count: ${verification.signatureCount}`);
  console.log(`   Validator Weight: ${verification.validatorWeightPercent.toFixed(1)}%`);
  
  if (verification.errors.length > 0) {
    console.log(`   Errors: ${verification.errors.join(', ')}`);
  }
  console.log('');

  console.log('6. Parsing transaction from URL...');
  const txPayload = txUrl.replace('/tx/', '');
  const parsedTx = decodeTransaction(txPayload);
  
  if (parsedTx) {
    console.log(`   Transaction verified from URL:`);
    console.log(`   - From: ${parsedTx.from}`);
    console.log(`   - To: ${parsedTx.to}`);
    console.log(`   - Amount: ${parsedTx.amount}`);
    console.log(`   - Timestamp: ${new Date(parsedTx.ts).toISOString()}`);
  }
  console.log('');

  console.log('=== Demo Complete ===\n');
  console.log('Key Insight:');
  console.log('The finalized URL contains EVERYTHING needed to verify:');
  console.log('  1. The transaction payload (embedded in URL)');
  console.log('  2. Parent transaction references (tipUrls - recursive crawling)');
  console.log('  3. Network consensus proof (checkpoint signatures)\n');
  console.log('A recipient can verify the entire financial history');
  console.log('using ONLY the URL - no nodes, no apps, just the link.\n');
  
  console.log('Finalized URL (copy this):');
  console.log(finalizedData.finalizedUrl!.slice(0, 200) + '...');
}

main().catch(console.error);
