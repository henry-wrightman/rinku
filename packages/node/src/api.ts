import express from 'express';
import cors from 'cors';
import { StateManager } from './state.js';
import { Consensus } from './consensus.js';
import { Mempool } from './mempool.js';
import { PeerSyncService } from './peerSync.js';
import { ContractService } from './contracts.js';
import { RewardsService } from './rewards.js';
import { CheckpointService } from './checkpoint.js';
import { GasService } from './gas.js';
import { EmissionService, SlashingService, TOKENOMICS_CONFIG } from './tokenomics.js';
import { FinalityMetricsService } from './finality.js';
import { GossipService, type GossipMessage } from './gossip.js';
import { ForkRemediationService } from './fork-remediation.js';
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

export interface TokenomicsServices {
  emissionService?: EmissionService;
  slashingService?: SlashingService;
}

export interface ForkServices {
  gossipService?: GossipService;
  forkRemediationService?: ForkRemediationService;
}

export function createAPI(
  state: StateManager,
  consensus: Consensus,
  mempool: Mempool,
  peerSync?: PeerSyncService,
  contractService?: ContractService,
  rewardsService?: RewardsService,
  checkpointService?: CheckpointService,
  gasService?: GasService,
  onTransaction?: () => Promise<void>,
  tokenomics?: TokenomicsServices,
  finalityMetrics?: FinalityMetricsService,
  forkServices?: ForkServices
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

  const TPS_WINDOW_MS = 60000;

  app.get('/api/stats/network', (_req, res) => {
    const now = Date.now();
    const nodes = consensus.getAllNodes();
    
    const cutoff = now - TPS_WINDOW_MS;
    const recentFromDag = nodes.filter(n => n.tx.ts > cutoff).length;
    const tps = recentFromDag / 60;
    
    let finalizedCount = 0;
    let unfinalizedCount = 0;
    for (const node of nodes) {
      if (consensus.hasFinality(node.tx.hash)) {
        finalizedCount++;
      } else {
        unfinalizedCount++;
      }
    }
    
    let checkpointCount = 0;
    let latestCheckpointHeight = 0;
    let latestCheckpointId: string | null = null;
    if (checkpointService) {
      const checkpoints = checkpointService.getAllCheckpoints();
      checkpointCount = checkpoints.length;
      const latest = checkpointService.getLatestCheckpoint();
      if (latest) {
        latestCheckpointHeight = latest.height;
        latestCheckpointId = latest.checkpointId;
      }
    }
    
    let totalStaked = 0;
    let validatorCount = 0;
    if (rewardsService) {
      totalStaked = rewardsService.getTotalStaked();
      validatorCount = rewardsService.getActiveValidators().length;
    }
    
    res.json({
      tps: Math.round(tps * 100) / 100,
      finalizedCount,
      unfinalizedCount,
      finalityRatio: nodes.length > 0 ? Math.round((finalizedCount / nodes.length) * 100) : 0,
      checkpointCount,
      latestCheckpointHeight,
      latestCheckpointId: latestCheckpointId ? latestCheckpointId.slice(0, 16) + '...' : null,
      totalStaked,
      validatorCount,
      networkAge: Math.round(process.uptime())
    });
  });

  app.get('/api/tips', (_req, res) => {
    const tips = consensus.getTips();
    res.json({ tips });
  });

  app.get('/api/gas/price', (_req, res) => {
    if (!gasService) {
      res.json({ current: 0, min: 0, max: 0, avgLast100: 0, lastUpdated: Date.now() });
      return;
    }
    res.json(gasService.getCurrentGasPrice());
  });

  app.get('/api/gas/stats', (_req, res) => {
    if (!gasService) {
      res.json({ totalBurned: 0, totalToValidators: 0, avgFee: 0, txCount: 0 });
      return;
    }
    res.json(gasService.getStats());
  });

  app.get('/api/gas/config', (_req, res) => {
    if (!gasService) {
      res.json({ minFee: 0, maxFee: 0, baseFee: 0, feeMultiplier: 1, burnPercent: 50, validatorPercent: 50 });
      return;
    }
    res.json(gasService.getConfig());
  });

  app.get('/api/finality/metrics', (_req, res) => {
    if (!finalityMetrics) {
      res.json({
        avgTimeToFinality: 0,
        medianTimeToFinality: 0,
        p95TimeToFinality: 0,
        pendingCount: 0,
        finalizedCount: 0,
        finalityRate: 1,
        checkpointLatency: 0,
        checkpointsPerMinute: 0,
        lastCheckpointAge: 0,
        txThroughput: 0
      });
      return;
    }
    res.json(finalityMetrics.getMetrics());
  });

  app.get('/api/tokenomics/supply', (_req, res) => {
    const checkpointHeight = checkpointService?.getLatestCheckpoint()?.height ?? 0;
    const gasStats = gasService?.getStats();
    const totalBurned = gasStats?.totalBurned ?? 0;
    
    if (tokenomics?.emissionService) {
      const stats = tokenomics.emissionService.getStats(checkpointHeight);
      const circulatingSupply = TOKENOMICS_CONFIG.GENESIS_ALLOCATION + stats.totalEmitted - totalBurned;
      res.json({
        maxSupply: TOKENOMICS_CONFIG.MAX_SUPPLY,
        genesisAllocation: TOKENOMICS_CONFIG.GENESIS_ALLOCATION,
        circulatingSupply: Math.max(0, circulatingSupply),
        totalEmitted: stats.totalEmitted,
        totalBurned,
        remainingToEmit: stats.remainingToEmit,
        currentReward: stats.currentReward,
        halvingEpoch: stats.halvingEpoch,
        nextHalvingAt: stats.nextHalvingAt,
        halvingInterval: TOKENOMICS_CONFIG.HALVING_INTERVAL,
        checkpointHeight
      });
    } else {
      res.json({
        maxSupply: TOKENOMICS_CONFIG.MAX_SUPPLY,
        genesisAllocation: TOKENOMICS_CONFIG.GENESIS_ALLOCATION,
        circulatingSupply: Math.max(0, TOKENOMICS_CONFIG.GENESIS_ALLOCATION - totalBurned),
        totalEmitted: 0,
        totalBurned,
        remainingToEmit: TOKENOMICS_CONFIG.MAX_SUPPLY - TOKENOMICS_CONFIG.GENESIS_ALLOCATION,
        currentReward: TOKENOMICS_CONFIG.INITIAL_CHECKPOINT_REWARD,
        halvingEpoch: 0,
        nextHalvingAt: TOKENOMICS_CONFIG.HALVING_INTERVAL,
        halvingInterval: TOKENOMICS_CONFIG.HALVING_INTERVAL,
        checkpointHeight
      });
    }
  });

  app.get('/api/tokenomics/emission', (_req, res) => {
    const checkpointHeight = checkpointService?.getLatestCheckpoint()?.height ?? 0;
    
    const schedule = [];
    for (let epoch = 0; epoch <= TOKENOMICS_CONFIG.HALVINGS_COUNT; epoch++) {
      const reward = TOKENOMICS_CONFIG.INITIAL_CHECKPOINT_REWARD / Math.pow(2, epoch);
      schedule.push({
        epoch,
        startHeight: epoch * TOKENOMICS_CONFIG.HALVING_INTERVAL,
        reward: Math.max(reward, TOKENOMICS_CONFIG.MIN_CHECKPOINT_REWARD)
      });
    }
    
    res.json({
      currentEpoch: Math.floor(checkpointHeight / TOKENOMICS_CONFIG.HALVING_INTERVAL),
      currentReward: tokenomics?.emissionService?.getCheckpointReward(checkpointHeight) ?? TOKENOMICS_CONFIG.INITIAL_CHECKPOINT_REWARD,
      halvingInterval: TOKENOMICS_CONFIG.HALVING_INTERVAL,
      totalHalvings: TOKENOMICS_CONFIG.HALVINGS_COUNT,
      minReward: TOKENOMICS_CONFIG.MIN_CHECKPOINT_REWARD,
      schedule,
      stakeWeightPercent: TOKENOMICS_CONFIG.STAKE_WEIGHT_PERCENT * 100,
      ageWeightPercent: TOKENOMICS_CONFIG.AGE_WEIGHT_PERCENT * 100
    });
  });

  app.get('/api/tokenomics/slashing', (_req, res) => {
    res.json({
      config: {
        doubleSignPercent: TOKENOMICS_CONFIG.SLASH_DOUBLE_SIGN_PERCENT * 100,
        invalidCheckpointPercent: TOKENOMICS_CONFIG.SLASH_INVALID_CHECKPOINT_PERCENT * 100,
        livenessPercent: TOKENOMICS_CONFIG.SLASH_LIVENESS_PERCENT * 100,
        livenessRepeatPercent: TOKENOMICS_CONFIG.SLASH_LIVENESS_REPEAT_PERCENT * 100,
        livenessMissThreshold: TOKENOMICS_CONFIG.LIVENESS_MISS_THRESHOLD,
        unbondingPeriodDays: TOKENOMICS_CONFIG.UNBONDING_PERIOD_MS / (24 * 60 * 60 * 1000)
      },
      events: tokenomics?.slashingService?.getSlashEvents(50) ?? [],
      totalSlashed: tokenomics?.slashingService?.getTotalSlashed() ?? 0,
      unbondingQueue: tokenomics?.slashingService?.getUnbondingQueue() ?? []
    });
  });

  app.get('/api/tokenomics/slashing/:validator', (req, res) => {
    const validator = req.params.validator;
    
    if (!tokenomics?.slashingService) {
      res.json({ validator, events: [], unbonding: [] });
      return;
    }
    
    res.json({
      validator,
      events: tokenomics.slashingService.getValidatorSlashHistory(validator),
      unbonding: tokenomics.slashingService.getUnbondingForValidator(validator)
    });
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

      const isFaucetOrGenesis = tx.from === 'faucet' || tx.from === 'genesis';
      if (gasService && !isFaucetOrGenesis) {
        const feeValidation = gasService.validateFee(tx.fee || 0);
        if (!feeValidation.valid) {
          res.status(400).json({ error: feeValidation.error });
          return;
        }
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

      if (forkServices?.gossipService) {
        forkServices.gossipService.broadcastTransaction(tx, pubKeyArray);
      }
      if (forkServices?.forkRemediationService) {
        forkServices.forkRemediationService.indexTransaction(tx);
      }

      if (finalityMetrics && tx.hash) {
        finalityMetrics.recordTxSubmission(tx.hash);
      }

      if (gasService && tx.fee && tx.fee > 0) {
        const { toValidators } = gasService.recordFee(tx.fee);
        
        if (rewardsService && toValidators > 0) {
          await rewardsService.distributeFeeToValidators(toValidators);
        }
      }

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
      const prunedInfo = consensus.getPrunedTxInfo(req.params.hash);
      if (prunedInfo) {
        res.status(410).json({ 
          error: 'Transaction pruned',
          pruned: true,
          checkpointId: prunedInfo.checkpointId,
          checkpointHeight: prunedInfo.checkpointHeight,
          prunedAt: prunedInfo.prunedAt
        });
        return;
      }
      res.status(404).json({ error: 'Transaction not found' });
      return;
    }
    res.json(node);
  });

  app.get('/api/tx/:hash/proof', async (req, res) => {
    const hash = req.params.hash;
    const node = consensus.getNode(hash);
    if (!node) {
      res.status(404).json({ error: 'Transaction not found' });
      return;
    }

    const getCheckpoint = checkpointService 
      ? (checkpointId: string) => {
          const checkpoint = checkpointService.getCheckpoint(checkpointId);
          if (!checkpoint) return null;
          return {
            checkpointId: checkpoint.checkpointId,
            merkleRoot: checkpoint.merkleRoot,
            txMerkleRoot: checkpoint.txMerkleRoot,
            height: checkpoint.height,
            signatureCount: checkpoint.signatures.length
          };
        }
      : undefined;

    const getMerkleProof = checkpointService
      ? async (txHash: string, checkpointId: string) => {
          return checkpointService.getTransactionMerkleProof(txHash, checkpointId);
        }
      : undefined;

    const proofUrl = await consensus.getSelfCrawlableUrl(hash, getCheckpoint, getMerkleProof);
    if (!proofUrl) {
      res.status(500).json({ error: 'Failed to generate proof URL' });
      return;
    }

    const bundle = await consensus.getSelfCrawlableBundle(hash, getCheckpoint, getMerkleProof);

    res.json({
      hash,
      proofUrl,
      bundle,
      hasFinality: consensus.hasFinality(hash),
      finality: node.finality
    });
  });

  type LightNode = { hash: string; from: string; to: string; amount: number; fee: number; ts: number; parentCount: number; url: string; weight: number; confirmed: boolean };
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
      fee: node.tx.fee || 0,
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

      if (forkServices?.gossipService) {
        forkServices.gossipService.broadcastTransaction(tx);
      }
      if (forkServices?.forkRemediationService) {
        forkServices.forkRemediationService.indexTransaction(tx);
      }

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
        fee: 0,
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

      const { result, receipt } = await contractService.executeCall(contractTx);

      res.json({
        success: result.success,
        stateDiff: result.stateDiff,
        gasUsed: result.gasUsed,
        logs: result.logs,
        error: result.error,
        newStateHash: result.stateDiff?.postHash,
        receipt: receipt ? {
          callId: receipt.callId,
          status: receipt.status,
          effectsHash: receipt.effectsHash,
          eventsHash: receipt.eventsHash,
          eventCount: receipt.eventCount
        } : undefined
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

  app.post('/api/gossip', async (req, res) => {
    if (!forkServices?.gossipService) {
      res.status(501).json({ error: 'Gossip not enabled' });
      return;
    }
    try {
      const message = req.body as GossipMessage;
      await forkServices.gossipService.receiveGossipMessage(message);
      res.json({ success: true });
    } catch (error: any) {
      res.status(400).json({ error: error.message });
    }
  });

  app.get('/api/gossip/stats', (_req, res) => {
    if (!forkServices?.gossipService) {
      res.status(501).json({ error: 'Gossip not enabled' });
      return;
    }
    res.json(forkServices.gossipService.getStats());
  });

  app.get('/api/fork/stats', (_req, res) => {
    if (!forkServices?.forkRemediationService) {
      res.status(501).json({ error: 'Fork remediation not enabled' });
      return;
    }
    res.json(forkServices.forkRemediationService.getStats());
  });

  app.get('/api/fork/double-spends', (_req, res) => {
    if (!forkServices?.forkRemediationService) {
      res.status(501).json({ error: 'Fork remediation not enabled' });
      return;
    }
    res.json({
      doubleSpends: forkServices.forkRemediationService.getDoubleSpends()
    });
  });

  app.get('/api/fork/events', (_req, res) => {
    if (!forkServices?.forkRemediationService) {
      res.status(501).json({ error: 'Fork remediation not enabled' });
      return;
    }
    res.json({
      forks: forkServices.forkRemediationService.getForkEvents()
    });
  });

  return app;
}
