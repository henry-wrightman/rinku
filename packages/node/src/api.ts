import express from 'express';
import cors from 'cors';
import { StateManager } from './state.js';
import { Consensus } from './consensus.js';
import { Mempool } from './mempool.js';
import { PeerSyncService } from './peerSync.js';
import { ContractService } from './contracts.js';
import { RewardsService } from './rewards.js';
import { CheckpointService } from './checkpoint.js';
import { 
  parseTransactionURL, 
  parseContractURL,
  createContractId,
  computeStateHash,
  embedProofInUrl,
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
  checkpointService?: CheckpointService,
  onTransaction?: () => Promise<void>
) {
  const app = express();

  app.use(cors());
  app.use(express.json({ limit: '50mb' }));

  app.use((err: any, _req: express.Request, res: express.Response, next: express.NextFunction) => {
    if (err.type === 'entity.too.large') {
      res.status(413).json({ error: 'Request payload too large' });
      return;
    }
    next(err);
  });

  app.get('/api/health', (_req, res) => {
    res.json({ status: 'ok', timestamp: Date.now() });
  });

  app.get('/api/stats', (_req, res) => {
    const memUsage = process.memoryUsage();
    res.json({
      dagSize: consensus.getAllNodes().length,
      tipCount: consensus.getTips().length,
      accountCount: state.getAllAccounts().size,
      mempoolSize: mempool.size(),
      memoryMB: {
        heapUsed: Math.round(memUsage.heapUsed / 1024 / 1024),
        heapTotal: Math.round(memUsage.heapTotal / 1024 / 1024),
        rss: Math.round(memUsage.rss / 1024 / 1024)
      },
      uptime: Math.round(process.uptime())
    });
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

      if (onTransaction) {
        onTransaction();
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

  app.get('/api/tx/:hash/proof', (req, res) => {
    const hash = req.params.hash;
    const node = consensus.getNode(hash);
    if (!node) {
      res.status(404).json({ error: 'Transaction not found' });
      return;
    }

    const isConfirmed = (txHash: string) => {
      const n = consensus.getNode(txHash);
      return n?.confirmed || false;
    };

    const getCheckpoint = checkpointService 
      ? () => {
          const latest = checkpointService.getLatestCheckpoint();
          if (!latest) return null;
          return {
            checkpointId: latest.checkpointId,
            merkleRoot: latest.merkleRoot,
            height: latest.height,
            signatureCount: latest.signatures.length
          };
        }
      : undefined;

    const proofUrl = consensus.getSelfCrawlableUrl(hash, isConfirmed, getCheckpoint);
    if (!proofUrl) {
      res.status(500).json({ error: 'Failed to generate proof URL' });
      return;
    }

    res.json({
      hash,
      proofUrl,
      bundle: consensus.getSelfCrawlableBundle(hash, isConfirmed, getCheckpoint)
    });
  });

  type LightNode = { hash: string; from: string; to: string; amount: number; ts: number; parentCount: number; url: string; weight: number; confirmed: boolean };
  let sortedNodesCache: { nodes: LightNode[]; lastSize: number; timestamp: number } | null = null;
  const CACHE_TTL = 5000;

  const getSortedNodes = (): LightNode[] => {
    const now = Date.now();
    const currentSize = consensus.getDAGSize();
    
    if (sortedNodesCache && 
        (now - sortedNodesCache.timestamp) < CACHE_TTL && 
        sortedNodesCache.lastSize === currentSize) {
      return sortedNodesCache.nodes;
    }
    
    const allNodes = consensus.getAllNodes();
    const lightNodes: LightNode[] = allNodes.map(node => ({
      hash: node.tx.hash,
      from: node.tx.from,
      to: node.tx.to,
      amount: node.tx.amount,
      ts: node.tx.ts,
      parentCount: node.tx.tipUrls?.length || 0,
      url: node.url || '',
      weight: node.weight,
      confirmed: node.confirmed
    }));
    
    lightNodes.sort((a, b) => b.ts - a.ts);
    
    sortedNodesCache = { nodes: lightNodes, lastSize: currentSize, timestamp: now };
    return lightNodes;
  };

  app.get('/api/dag/summary', (_req, res) => {
    const tipCount = consensus.getTips().length;
    res.json({
      totalNodes: consensus.getDAGSize(),
      tipCount,
      merkleRoot: state.getMerkleRoot(),
      accountCount: state.getAllAccounts().size
    });
  });

  app.get('/api/dag', (req, res) => {
    const page = parseInt(req.query.page as string) || 0;
    const limit = Math.min(parseInt(req.query.limit as string) || 20, 50);
    
    const sorted = getSortedNodes();
    const pageNodes = sorted.slice(page * limit, (page + 1) * limit);
    
    res.json({
      nodes: pageNodes,
      totalNodes: sorted.length,
      page,
      limit,
      hasMore: (page + 1) * limit < sorted.length
    });
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

      if (onTransaction) {
        onTransaction();
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

  app.post('/api/sync/announce', express.json(), (req, res) => {
    if (!peerSync) {
      res.status(501).json({ error: 'Peer sync not enabled' });
      return;
    }

    const config = peerSync.getDiscoveryConfig();
    if (!config.announceEnabled) {
      res.status(403).json({ error: 'Peer announcements are disabled on this node' });
      return;
    }

    const { url, nodeId } = req.body;
    if (!url || typeof url !== 'string') {
      res.status(400).json({ error: 'Missing or invalid url field' });
      return;
    }

    const added = peerSync.addPeer(url, 'announce');
    res.json({ 
      accepted: added,
      peerCount: peerSync.getPeerCount(),
      maxPeers: config.maxPeers
    });
  });

  app.get('/api/sync/discovery', (_req, res) => {
    if (!peerSync) {
      res.status(501).json({ error: 'Peer sync not enabled' });
      return;
    }
    res.json({
      config: peerSync.getDiscoveryConfig(),
      peerCount: peerSync.getPeerCount(),
      onlinePeers: peerSync.getOnlinePeers().length
    });
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

  app.post('/api/rewards/:address/claim', async (req, res) => {
    if (!rewardsService) {
      res.status(501).json({ error: 'Rewards not enabled' });
      return;
    }
    const result = await rewardsService.claimRewards(req.params.address);
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

  app.post('/api/staking/stake', async (req, res) => {
    if (!rewardsService) {
      res.status(501).json({ error: 'Rewards not enabled' });
      return;
    }

    const { address, amount } = req.body as { address: string; amount: number };
    if (!address || !amount) {
      res.status(400).json({ error: 'Address and amount required' });
      return;
    }

    const result = await rewardsService.stake(address, amount);
    res.json(result);
  });

  app.post('/api/staking/unstake', async (req, res) => {
    if (!rewardsService) {
      res.status(501).json({ error: 'Rewards not enabled' });
      return;
    }

    const { address } = req.body as { address: string };
    if (!address) {
      res.status(400).json({ error: 'Address required' });
      return;
    }

    const result = await rewardsService.unstake(address);
    res.json(result);
  });

  // ============================================
  // Checkpoint & Finality Endpoints
  // ============================================

  app.get('/api/checkpoints', (_req, res) => {
    if (!checkpointService) {
      res.status(501).json({ error: 'Checkpoints not enabled' });
      return;
    }
    const checkpoints = checkpointService.getAllCheckpoints();
    const latest = checkpointService.getLatestCheckpoint();
    res.json({
      checkpoints: checkpoints.slice(0, 20),
      latest: latest?.checkpointId || null,
      totalCount: checkpoints.length
    });
  });

  app.get('/api/checkpoints/genesis', (_req, res) => {
    if (!checkpointService) {
      res.status(501).json({ error: 'Checkpoints not enabled' });
      return;
    }
    const genesisConfig = checkpointService.getGenesisConfig();
    if (!genesisConfig) {
      res.status(404).json({ error: 'Genesis not initialized' });
      return;
    }
    res.json(genesisConfig);
  });

  app.get('/api/checkpoints/chain', (_req, res) => {
    if (!checkpointService) {
      res.status(501).json({ error: 'Checkpoints not enabled' });
      return;
    }
    const chain = checkpointService.getCheckpointChain();
    res.json({ chain, length: chain.length });
  });

  app.get('/api/checkpoints/latest', (_req, res) => {
    if (!checkpointService) {
      res.status(501).json({ error: 'Checkpoints not enabled' });
      return;
    }
    const latest = checkpointService.getLatestCheckpoint();
    if (!latest) {
      res.status(404).json({ error: 'No checkpoints yet' });
      return;
    }
    const proof = checkpointService.getCheckpointProof();
    const finalized = checkpointService.isCheckpointFinalized(latest.checkpointId);
    res.json({ checkpoint: latest, proof, finalized });
  });

  app.get('/api/checkpoints/:id', (req, res) => {
    if (!checkpointService) {
      res.status(501).json({ error: 'Checkpoints not enabled' });
      return;
    }
    const checkpoint = checkpointService.getCheckpoint(req.params.id);
    if (!checkpoint) {
      res.status(404).json({ error: 'Checkpoint not found' });
      return;
    }
    const proof = checkpointService.getCheckpointProof(req.params.id);
    const finalized = checkpointService.isCheckpointFinalized(req.params.id);
    res.json({ checkpoint, proof, finalized });
  });

  app.post('/api/checkpoints/create', async (_req, res) => {
    if (!checkpointService) {
      res.status(501).json({ error: 'Checkpoints not enabled' });
      return;
    }
    try {
      const checkpoint = await checkpointService.createCheckpoint();
      res.json({ success: true, checkpoint });
    } catch (error: any) {
      res.status(500).json({ error: error.message });
    }
  });

  app.get('/api/tx/:hash/finalized', (req, res) => {
    if (!checkpointService) {
      res.status(501).json({ error: 'Checkpoints not enabled' });
      return;
    }
    const node = consensus.getNode(req.params.hash);
    if (!node) {
      res.status(404).json({ error: 'Transaction not found' });
      return;
    }
    const proof = checkpointService.getCheckpointProof();
    if (!proof) {
      res.json({ finalized: false, reason: 'No checkpoints yet' });
      return;
    }
    const txUrl = node.url || '';
    const finalizedUrl = txUrl ? embedProofInUrl(txUrl, proof) : '';
    res.json({
      finalized: true,
      txUrl,
      finalizedUrl,
      proof
    });
  });

  return app;
}
