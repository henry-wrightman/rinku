import express from 'express';
import cors from 'cors';
import { StateManager } from './state.js';
import { Consensus } from './consensus.js';
import { Mempool } from './mempool.js';
import { PeerSyncService } from './peerSync.js';
import { ContractService } from './contracts.js';
import { RewardsService } from './rewards.js';
import { 
  parseTransactionURL, 
  createTransactionURL, 
  parseContractURL,
  createContractURL,
  createContractId,
  computeStateHash,
  type SignedTransaction,
  type ContractDeploy,
  type ContractTransaction
} from '@rinku/core';

export function createAPI(
  state: StateManager,
  consensus: Consensus,
  mempool: Mempool,
  peerSync?: PeerSyncService,
  contractService?: ContractService,
  rewardsService?: RewardsService,
  onTransaction?: () => Promise<void>
) {
  const app = express();

  app.use(cors());
  app.use(express.json({ limit: '10mb' }));

  app.get('/api/health', (_req, res) => {
    res.json({ status: 'ok', timestamp: Date.now() });
  });

  app.get('/api/tips', (_req, res) => {
    const tips = consensus.getTips();
    res.json({ tips });
  });

  app.get('/api/tipUrls', (_req, res) => {
    const tipUrls = consensus.getTipUrls();
    res.json({ tipUrls });
  });

  app.get('/api/account/:fingerprint', (req, res) => {
    const account = state.getAccount(req.params.fingerprint);
    if (!account) {
      res.status(404).json({ error: 'Account not found' });
      return;
    }
    res.json(account);
  });

  app.get('/api/accounts', (_req, res) => {
    const accounts = Array.from(state.getAllAccounts().entries()).map(
      ([fp, account]) => ({
        fingerprint: fp,
        balance: account.balance,
        nonce: account.nonce,
        firstTxTimestamp: account.firstTxTimestamp
      })
    );
    res.json({ accounts });
  });

  app.post('/api/tx', async (req, res) => {
    try {
      const { tx, publicKey } = req.body as {
        tx: SignedTransaction;
        publicKey?: number[];
      };

      if (!tx) {
        res.status(400).json({ error: 'Transaction required' });
        return;
      }

      const pubKeyArray = publicKey ? new Uint8Array(publicKey) : undefined;
      
      if (pubKeyArray) {
        consensus.registerPublicKey(tx.from, pubKeyArray);
      }

      const validation = await consensus.validateTransaction(
        tx,
        state.getAllAccounts(),
        pubKeyArray
      );

      if (!validation.valid) {
        res.status(400).json({ error: validation.error });
        return;
      }

      const applied = await state.applyTransaction(tx);
      if (!applied) {
        res.status(400).json({ error: 'Failed to apply transaction' });
        return;
      }

      await consensus.addTransaction(tx);
      consensus.updateWeights(state.getAllAccounts());

      if (onTransaction) {
        await onTransaction();
      }

      res.json({
        success: true,
        hash: tx.hash,
        merkleRoot: state.getMerkleRoot()
      });
    } catch (error: any) {
      res.status(500).json({ error: error.message });
    }
  });

  app.get('/api/tx/:hash', (req, res) => {
    const node = consensus.getNode(req.params.hash);
    if (!node) {
      res.status(404).json({ error: 'Transaction not found' });
      return;
    }
    res.json(node);
  });

  app.get('/api/dag', (_req, res) => {
    const nodes = consensus.getAllNodes().map(node => ({
      ...node,
      url: createTransactionURL(node.tx).path
    }));
    const tips = consensus.getTips();
    const tipUrls = consensus.getTipUrls();
    res.json({ nodes, tips, tipUrls, merkleRoot: state.getMerkleRoot() });
  });

  app.get('/api/state', (_req, res) => {
    res.json({
      accounts: Array.from(state.getAllAccounts().entries()),
      merkleRoot: state.getMerkleRoot(),
      tips: consensus.getTips()
    });
  });

  app.get('/tx/:payload', async (req, res) => {
    const url = `/tx/${req.params.payload}`;
    const tx = parseTransactionURL(url) as SignedTransaction | null;
    if (!tx) {
      res.status(400).json({ error: 'Invalid transaction URL' });
      return;
    }

    const existingNode = consensus.getNode(tx.hash);
    if (existingNode) {
      res.json({
        status: 'exists',
        message: 'Transaction already in DAG',
        tx: existingNode.tx,
        url
      });
      return;
    }

    const submit = req.query.submit === 'true' || req.query.submit === '1';
    
    if (!submit) {
      res.json({
        status: 'preview',
        message: 'Add ?submit=true&pubkey=<base64> to submit this transaction',
        tx,
        url
      });
      return;
    }

    try {
      let pubKeyArray: Uint8Array | undefined;
      
      if (typeof req.query.pubkey === 'string') {
        try {
          const pubkeyB64 = req.query.pubkey.replace(/-/g, '+').replace(/_/g, '/');
          pubKeyArray = new Uint8Array(Buffer.from(pubkeyB64, 'base64'));
          consensus.registerPublicKey(tx.from, pubKeyArray);
        } catch {
        }
      }

      const skipValidation = tx.from === 'genesis' || tx.from === 'faucet';
      
      if (!skipValidation) {
        if (!pubKeyArray) {
          res.status(400).json({ 
            status: 'invalid',
            error: 'Public key required for signature verification. Add &pubkey=<base64url> to the URL.',
            tx,
            url
          });
          return;
        }
        
        const validation = await consensus.validateTransaction(
          tx,
          state.getAllAccounts(),
          pubKeyArray
        );

        if (!validation.valid) {
          res.status(400).json({ 
            status: 'invalid',
            error: validation.error,
            tx,
            url
          });
          return;
        }
      }

      const applied = await state.applyTransaction(tx, { skipChecks: skipValidation });
      if (!applied) {
        res.status(400).json({ 
          status: 'failed',
          error: 'Failed to apply transaction',
          tx,
          url
        });
        return;
      }

      await consensus.addTransaction(tx);
      consensus.updateWeights(state.getAllAccounts());

      if (onTransaction) {
        await onTransaction();
      }

      res.json({
        status: 'submitted',
        message: 'Transaction added to DAG',
        hash: tx.hash,
        merkleRoot: state.getMerkleRoot(),
        tx,
        url
      });
    } catch (error: any) {
      res.status(500).json({ 
        status: 'error',
        error: error.message,
        tx,
        url
      });
    }
  });

  // ============================================
  // Smart Contract Endpoints
  // ============================================

  app.get('/api/contracts', (_req, res) => {
    if (!contractService) {
      res.status(501).json({ error: 'Contract service not enabled' });
      return;
    }
    const contracts = contractService.getAllContracts().map(c => ({
      contractId: c.contractId,
      creator: c.creator,
      deployUrl: c.deployUrl,
      stateHash: c.stateHash,
      height: c.height,
      createdAt: c.createdAt
    }));
    res.json({ contracts });
  });

  app.get('/api/contracts/:contractId', (req, res) => {
    if (!contractService) {
      res.status(501).json({ error: 'Contract service not enabled' });
      return;
    }
    const contract = contractService.getContract(req.params.contractId);
    if (!contract) {
      res.status(404).json({ error: 'Contract not found' });
      return;
    }
    res.json(contract);
  });

  app.get('/api/contracts/:contractId/state', (req, res) => {
    if (!contractService) {
      res.status(501).json({ error: 'Contract service not enabled' });
      return;
    }
    const contractState = contractService.getContractState(req.params.contractId);
    if (!contractState) {
      res.status(404).json({ error: 'Contract not found' });
      return;
    }
    res.json({ state: contractState, stateHash: computeStateHash(contractState) });
  });

  app.get('/api/contracts/:contractId/history', (req, res) => {
    if (!contractService) {
      res.status(501).json({ error: 'Contract service not enabled' });
      return;
    }
    const history = contractService.getExecutionHistory(req.params.contractId);
    res.json({ history });
  });

  app.post('/api/contracts/deploy', async (req, res) => {
    if (!contractService) {
      res.status(501).json({ error: 'Contract service not enabled' });
      return;
    }

    try {
      const { creator, wasmBase64, initState, tipUrls, sig } = req.body;
      
      if (!creator || !wasmBase64) {
        res.status(400).json({ error: 'creator and wasmBase64 required' });
        return;
      }

      const creatorAccount = state.getAccount(creator);
      const nonce = creatorAccount ? creatorAccount.nonce + 1 : 1;
      const contractId = createContractId(creator, nonce);

      const deploy: ContractDeploy = {
        type: 'deploy',
        contractId,
        creator,
        wasmBase64,
        initState: initState || {},
        tipUrls: tipUrls || consensus.getTipUrls(),
        sig: sig || '',
        ts: Date.now()
      };

      const result = await contractService.deployContract(deploy);
      
      if (!result.success) {
        res.status(400).json({ error: result.error });
        return;
      }

      res.json({
        success: true,
        contractId: result.contractId,
        deployUrl: result.deployUrl
      });
    } catch (error: any) {
      res.status(500).json({ error: error.message });
    }
  });

  app.post('/api/contracts/:contractId/call', async (req, res) => {
    if (!contractService) {
      res.status(501).json({ error: 'Contract service not enabled' });
      return;
    }

    try {
      const { contractId } = req.params;
      const { entrypoint, input, caller } = req.body;

      if (!entrypoint) {
        res.status(400).json({ error: 'entrypoint required' });
        return;
      }

      const contract = contractService.getContract(contractId);
      if (!contract) {
        res.status(404).json({ error: 'Contract not found' });
        return;
      }

      const simulation = contractService.simulateCall(contractId, entrypoint, input || {});
      
      if (!simulation.success) {
        res.status(400).json({ 
          error: simulation.error,
          logs: simulation.logs,
          gasUsed: simulation.gasUsed
        });
        return;
      }

      const postStateHash = simulation.stateDiff?.postHash || contract.stateHash;

      const contractTx: ContractTransaction = {
        from: caller || 'contract-caller',
        to: contractId,
        amount: 0,
        nonce: Date.now(),
        tipUrls: consensus.getTipUrls(),
        sig: '',
        ts: Date.now(),
        hash: '',
        contract: {
          action: 'call',
          contractId,
          entrypoint,
          input: input || {},
          preStateHash: contract.stateHash,
          postStateHash
        }
      };

      const result = await contractService.executeCall(contractTx);

      res.json({
        success: result.success,
        stateDiff: result.stateDiff,
        gasUsed: result.gasUsed,
        logs: result.logs,
        error: result.error,
        newStateHash: result.stateDiff?.postHash
      });
    } catch (error: any) {
      res.status(500).json({ error: error.message });
    }
  });

  app.post('/api/contracts/:contractId/simulate', (req, res) => {
    if (!contractService) {
      res.status(501).json({ error: 'Contract service not enabled' });
      return;
    }

    const { contractId } = req.params;
    const { entrypoint, input } = req.body;

    if (!entrypoint) {
      res.status(400).json({ error: 'entrypoint required' });
      return;
    }

    const result = contractService.simulateCall(contractId, entrypoint, input || {});
    res.json(result);
  });

  app.get('/sc/:payload', (req, res) => {
    const url = `/sc/${req.params.payload}`;
    const deploy = parseContractURL(url);
    
    if (!deploy) {
      res.status(400).json({ error: 'Invalid contract URL' });
      return;
    }

    if (!contractService) {
      res.json({
        status: 'preview',
        message: 'Contract service not enabled',
        deploy,
        url
      });
      return;
    }

    const existing = contractService.getContract(deploy.contractId);
    if (existing) {
      res.json({
        status: 'deployed',
        contract: existing,
        url
      });
      return;
    }

    res.json({
      status: 'preview',
      message: 'Contract not yet deployed. POST to /api/contracts/deploy to deploy.',
      deploy,
      url
    });
  });

  // ============================================
  // Sync Endpoints
  // ============================================

  app.get('/api/sync/status', (_req, res) => {
    if (!peerSync) {
      res.status(501).json({ error: 'Peer sync not enabled' });
      return;
    }
    res.json(peerSync.getStatus());
  });

  app.get('/api/sync/transactions', (_req, res) => {
    const nodes = consensus.getAllNodes();
    const publicKeys = consensus.getPublicKeys();
    
    const transactions = nodes.map(node => ({
      tx: node.tx,
      publicKey: publicKeys.has(node.tx.from) 
        ? Array.from(publicKeys.get(node.tx.from)!)
        : undefined
    }));

    res.json({ transactions });
  });

  app.get('/api/sync/peers', (_req, res) => {
    if (!peerSync) {
      res.status(501).json({ error: 'Peer sync not enabled' });
      return;
    }
    res.json({ peers: peerSync.getPeers() });
  });

  app.post('/api/sync/force', async (_req, res) => {
    if (!peerSync) {
      res.status(501).json({ error: 'Peer sync not enabled' });
      return;
    }
    
    try {
      const result = await peerSync.forceSync();
      res.json(result);
    } catch (error: any) {
      res.status(500).json({ error: error.message });
    }
  });

  // ============================================
  // Rewards & Staking Endpoints
  // ============================================

  app.get('/api/rewards/config', (_req, res) => {
    if (!rewardsService) {
      res.status(501).json({ error: 'Rewards not enabled' });
      return;
    }
    res.json(rewardsService.getConfig());
  });

  app.get('/api/rewards/:address', (req, res) => {
    if (!rewardsService) {
      res.status(501).json({ error: 'Rewards not enabled' });
      return;
    }
    const summary = rewardsService.getRewardsSummary(req.params.address);
    res.json(summary);
  });

  app.post('/api/rewards/:address/claim', (req, res) => {
    if (!rewardsService) {
      res.status(501).json({ error: 'Rewards not enabled' });
      return;
    }
    const result = rewardsService.claimRewards(req.params.address);
    res.json(result);
  });

  app.get('/api/staking', (_req, res) => {
    if (!rewardsService) {
      res.status(501).json({ error: 'Rewards not enabled' });
      return;
    }
    res.json({
      totalStaked: rewardsService.getTotalStaked(),
      validators: rewardsService.getActiveValidators(),
      topStakers: rewardsService.getTopStakers(10),
      config: rewardsService.getConfig()
    });
  });

  app.get('/api/staking/:address', (req, res) => {
    if (!rewardsService) {
      res.status(501).json({ error: 'Rewards not enabled' });
      return;
    }
    const status = rewardsService.getStakingStatus(req.params.address);
    res.json(status);
  });

  app.post('/api/staking/stake', (req, res) => {
    if (!rewardsService) {
      res.status(501).json({ error: 'Rewards not enabled' });
      return;
    }

    const { address, amount } = req.body as { address: string; amount: number };
    if (!address || !amount) {
      res.status(400).json({ error: 'Address and amount required' });
      return;
    }

    const result = rewardsService.stake(address, amount);
    res.json(result);
  });

  app.post('/api/staking/unstake', (req, res) => {
    if (!rewardsService) {
      res.status(501).json({ error: 'Rewards not enabled' });
      return;
    }

    const { address } = req.body as { address: string };
    if (!address) {
      res.status(400).json({ error: 'Address required' });
      return;
    }

    const result = rewardsService.unstake(address);
    res.json(result);
  });

  return app;
}
