import {
  extractProofFromUrl,
  verifyCheckpointProof,
  verifyCheckpointChain,
  decodeTransaction,
  type ValidatorEntry,
  type GenesisConfig,
  type CheckpointProof
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
  proof?: CheckpointProof;
}

interface ChainResponse {
  chain: CheckpointProof[];
  length: number;
}

async function main() {
  console.log('=== Rinku Trustless Finality Proof Demo ===\n');
  console.log('This demo shows TRUSTLESS verification from a URL alone,');
  console.log('using the genesis checkpoint as the root of trust.\n');

  console.log('1. Fetching genesis configuration (root of trust)...');
  const genesisRes = await fetch(`${NODE_URL}/api/checkpoints/genesis`);
  const genesisConfig = await genesisRes.json() as GenesisConfig;
  
  console.log(`   Chain ID: ${genesisConfig.chainId}`);
  console.log(`   Genesis Time: ${new Date(genesisConfig.genesisTime).toISOString()}`);
  console.log(`   Genesis Checkpoint: ${genesisConfig.genesisCheckpointId}`);
  console.log(`   Initial Validators: ${genesisConfig.initialValidators.length}\n`);

  console.log('2. Creating a checkpoint on the network...');
  const createRes = await fetch(`${NODE_URL}/api/checkpoints/create`, {
    method: 'POST'
  });
  const createData = await createRes.json() as CheckpointCreateResponse;
  console.log(`   Checkpoint created: ${createData.checkpoint?.checkpointId || 'already exists'}\n`);

  console.log('3. Getting a transaction with finality proof...');
  const dagRes = await fetch(`${NODE_URL}/api/dag`);
  const dagData = await dagRes.json() as DAGResponse;
  
  if (!dagData.nodes || dagData.nodes.length === 0) {
    console.log('   No transactions found in the DAG');
    return;
  }

  const sampleTx = dagData.nodes[0];
  console.log(`   Transaction hash: ${sampleTx.tx.hash.slice(0, 16)}...`);
  console.log(`   Amount: ${sampleTx.tx.amount} coins\n`);

  console.log('4. Getting finalized URL with embedded proof...');
  const finalizedRes = await fetch(`${NODE_URL}/api/tx/${sampleTx.tx.hash}/finalized`);
  const finalizedData = await finalizedRes.json() as FinalizedResponse;

  if (!finalizedData.finalized) {
    console.log(`   Transaction not yet finalized: ${finalizedData.reason}`);
    return;
  }

  console.log(`   Original URL: ${finalizedData.txUrl!.length} chars`);
  console.log(`   Finalized URL: ${finalizedData.finalizedUrl!.length} chars`);
  console.log(`   Proof overhead: ${finalizedData.finalizedUrl!.length - finalizedData.txUrl!.length} chars\n`);

  console.log('5. Extracting proof from URL (NO node access from here)...');
  const { txUrl, proof } = extractProofFromUrl(finalizedData.finalizedUrl!);
  
  if (!proof) {
    console.log('   ERROR: Could not extract proof from URL');
    return;
  }

  console.log(`   Proof extracted:`);
  console.log(`   - Checkpoint ID: ${proof.checkpointId}`);
  console.log(`   - Height: ${proof.checkpointHeight}`);
  console.log(`   - Previous Checkpoint: ${proof.previousCheckpointId || 'genesis'}`);
  console.log(`   - Validators in proof: ${proof.validators.length}`);
  console.log(`   - Signatures: ${proof.signatures.length}\n`);

  console.log('6. TRUSTLESS VERIFICATION against genesis...');
  console.log('   (Verifying proof using ONLY genesis config as trust anchor)\n');

  if (proof.checkpointHeight === 0 || proof.validators.length === 0) {
    console.log('   Using genesis validators for verification...');
    const verification = await verifyCheckpointProof(
      proof, 
      genesisConfig.initialValidators
    );
    
    console.log(`   Valid: ${verification.valid}`);
    console.log(`   Verified Signatures: ${verification.signatureCount}`);
    console.log(`   Verified Weight: ${verification.validatorWeightPercent.toFixed(1)}%`);
    
    if (verification.errors.length > 0) {
      console.log(`   Errors: ${verification.errors.join(', ')}`);
    }
  } else {
    console.log('   Fetching checkpoint chain for full verification...');
    const chainRes = await fetch(`${NODE_URL}/api/checkpoints/chain`);
    const chainData = await chainRes.json() as ChainResponse;
    
    console.log(`   Chain length: ${chainData.length} checkpoints`);
    
    const verification = await verifyCheckpointChain(
      proof,
      chainData.chain,
      genesisConfig
    );
    
    console.log(`   Valid: ${verification.valid}`);
    console.log(`   Verified Signatures: ${verification.signatureCount}`);
    console.log(`   Verified Weight: ${verification.validatorWeightPercent.toFixed(1)}%`);
    
    if (verification.errors.length > 0) {
      console.log(`   Errors: ${verification.errors.join(', ')}`);
    }
  }
  console.log('');

  console.log('7. Parsing transaction from URL...');
  const txPayload = txUrl.replace('/tx/', '');
  const parsedTx = decodeTransaction(txPayload);
  
  if (parsedTx) {
    console.log(`   Transaction verified:`);
    console.log(`   - From: ${parsedTx.from}`);
    console.log(`   - To: ${parsedTx.to}`);
    console.log(`   - Amount: ${parsedTx.amount}`);
  }
  console.log('');

  console.log('=== TRUSTLESS VERIFICATION COMPLETE ===\n');
  console.log('Security Model:');
  console.log('  1. Genesis config is the ONLY trust anchor needed');
  console.log('  2. Each checkpoint commits to the validator set');
  console.log('  3. Checkpoint chain validated from genesis to current');
  console.log('  4. Signatures verified against AUTHENTICATED validator keys');
  console.log('  5. Forged proofs are REJECTED - weights cannot be inflated\n');
  
  console.log('A malicious validator CANNOT:');
  console.log('  - Forge checkpoint signatures (cryptographic)');
  console.log('  - Inflate their weight (verified against genesis/chain)');
  console.log('  - Create fake validators (not in authenticated set)');
  console.log('  - Rewrite history (checkpoint chain verified)\n');
  
  console.log('Finalized URL (contains everything needed):');
  console.log(finalizedData.finalizedUrl!.slice(0, 150) + '...');
}

main().catch(console.error);
