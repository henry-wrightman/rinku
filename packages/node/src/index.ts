import { createAPI } from './api.js';
import { StateManager } from './state.js';
import { Consensus } from './consensus.js';
import { Mempool } from './mempool.js';
import { hashTransaction, type SignedTransaction } from '@rinku/core';

const PORT = parseInt(process.env.NODE_PORT || '3001', 10);
const FAUCET_BALANCE = 1000000;

async function main() {
  console.log('Starting Rinku Node...');

  const state = new StateManager();
  const consensus = new Consensus();
  const mempool = new Mempool();

  const genesisTx: SignedTransaction = {
    from: 'genesis',
    to: 'faucet',
    amount: FAUCET_BALANCE,
    nonce: 0,
    tips: [],
    sig: 'genesis-signature',
    ts: Date.now(),
    hash: ''
  };
  genesisTx.hash = await hashTransaction(genesisTx);
  
  await state.applyTransaction(genesisTx);
  await consensus.addTransaction(genesisTx);
  consensus.updateWeights(state.getAllAccounts());
  
  console.log(`Genesis transaction created: ${genesisTx.hash.slice(0, 16)}...`);
  console.log(`Faucet account initialized with ${FAUCET_BALANCE} coins`);

  const app = createAPI(state, consensus, mempool);

  app.listen(PORT, '0.0.0.0', () => {
    console.log(`Rinku Node running on port ${PORT}`);
    console.log(`API available at http://0.0.0.0:${PORT}/api`);
  });
}

main().catch(console.error);

export { StateManager } from './state.js';
export { Consensus } from './consensus.js';
export { Mempool } from './mempool.js';
export { createAPI } from './api.js';
