const NODE_URL = process.env.RINKU_NODE_URL || 'http://localhost:3001';

async function main() {
  console.log('=== Rinku Smart Contract Demo ===\n');

  console.log('1. Deploying a token contract...');
  const deployRes = await fetch(`${NODE_URL}/api/contracts/deploy`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      creator: 'demo-deployer',
      wasmBase64: Buffer.from('mock-token-contract').toString('base64'),
      initState: { 
        name: 'Demo Token',
        symbol: 'DEMO',
        balances: {}
      }
    })
  });

  const deployData = await deployRes.json() as { success: boolean; contractId: string; deployUrl: string; error?: string };
  
  if (!deployData.success) {
    console.error('Deploy failed:', deployData.error);
    return;
  }

  console.log(`   Contract deployed: ${deployData.contractId}`);
  console.log(`   Deploy URL: ${deployData.deployUrl}\n`);

  const contractId = deployData.contractId;

  console.log('2. Minting tokens to alice...');
  const mintRes = await fetch(`${NODE_URL}/api/contracts/${contractId}/call`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      entrypoint: 'mint',
      input: { to: 'alice', amount: 1000 },
      caller: 'demo-minter'
    })
  });

  const mintData = await mintRes.json() as { success: boolean; gasUsed: number; logs: string[]; error?: string };
  console.log(`   Success: ${mintData.success}, Gas: ${mintData.gasUsed}`);
  console.log(`   Logs: ${mintData.logs?.join(', ')}\n`);

  console.log('3. Minting tokens to bob...');
  const mintBobRes = await fetch(`${NODE_URL}/api/contracts/${contractId}/call`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      entrypoint: 'mint',
      input: { to: 'bob', amount: 500 },
      caller: 'demo-minter'
    })
  });

  const mintBobData = await mintBobRes.json() as { success: boolean; logs: string[] };
  console.log(`   Success: ${mintBobData.success}`);
  console.log(`   Logs: ${mintBobData.logs?.join(', ')}\n`);

  console.log('4. Transferring tokens from alice to charlie...');
  const transferRes = await fetch(`${NODE_URL}/api/contracts/${contractId}/call`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      entrypoint: 'transfer',
      input: { from: 'alice', to: 'charlie', amount: 250 },
      caller: 'alice'
    })
  });

  const transferData = await transferRes.json() as { success: boolean; logs: string[] };
  console.log(`   Success: ${transferData.success}`);
  console.log(`   Logs: ${transferData.logs?.join(', ')}\n`);

  console.log('5. Checking final state...');
  const stateRes = await fetch(`${NODE_URL}/api/contracts/${contractId}/state`);
  const stateData = await stateRes.json() as { state: Record<string, unknown>; stateHash: string };
  
  console.log('   Contract State:');
  console.log(JSON.stringify(stateData.state, null, 4));
  console.log(`   State Hash: ${stateData.stateHash}\n`);

  console.log('6. Getting balance of alice (read-only)...');
  const balanceRes = await fetch(`${NODE_URL}/api/contracts/${contractId}/simulate`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      entrypoint: 'get_balance',
      input: { address: 'alice' }
    })
  });

  const balanceData = await balanceRes.json() as { success: boolean; logs: string[]; gasUsed: number };
  console.log(`   Logs: ${balanceData.logs?.join(', ')}`);
  console.log(`   Gas: ${balanceData.gasUsed}\n`);

  console.log('=== Demo Complete ===');
  console.log(`\nVisit the explorer and click "contracts" tab to see your deployed contract!`);
}

main().catch(console.error);
