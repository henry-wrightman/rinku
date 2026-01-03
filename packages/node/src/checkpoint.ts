import {
  type Checkpoint,
  type CheckpointProof,
  type ValidatorSignature,
  type ValidatorEntry,
  type CheckpointConfig,
  type GenesisConfig,
  createCheckpoint,
  createGenesisCheckpoint,
  createCheckpointProof,
  getCheckpointSigningData,
  computeValidatorSetHash,
  DEFAULT_CHECKPOINT_CONFIG,
  GENESIS_CHECKPOINT_ID,
  sign,
  verify,
  getTransactionMerkleRoot
} from '@rinku/core';

export interface CheckpointServiceDeps {
  getMerkleRoot: () => string;
  getTipCount: () => number;
  getTotalTransactions: () => number;
  getValidatorEntries: () => ValidatorEntry[];
  getTotalWeight: () => number;
  getPublicKey: (address: string) => Uint8Array | undefined;
  getPrivateKey: () => Uint8Array | undefined;
  getNodeAddress: () => string;
  getAllTransactionHashes?: () => string[];
  getStateRoot?: () => Promise<string>;
  getReceiptRoot?: () => Promise<string>;
}

export class CheckpointService {
  private checkpoints: Map<string, Checkpoint> = new Map();
  private latestCheckpoint: Checkpoint | null = null;
  private genesisCheckpoint: Checkpoint | null = null;
  private checkpointHeight = 0;
  private readonly maxCheckpoints = 10;
  private config: CheckpointConfig;
  private intervalId: NodeJS.Timeout | null = null;
  private chainId: string;
  private onCheckpointCallback?: (checkpointId: string, height: number) => void;

  constructor(
    private deps: CheckpointServiceDeps,
    chainId: string = 'rinku-testnet',
    config?: Partial<CheckpointConfig>
  ) {
    this.config = { ...DEFAULT_CHECKPOINT_CONFIG, ...config };
    this.chainId = chainId;
  }

  onCheckpoint(callback: (checkpointId: string, height: number) => void): void {
    this.onCheckpointCallback = callback;
  }

  getConfig(): CheckpointConfig {
    return { ...this.config };
  }

  getChainId(): string {
    return this.chainId;
  }

  async initializeGenesis(): Promise<Checkpoint> {
    const validators = this.deps.getValidatorEntries();
    const genesis = await createGenesisCheckpoint(this.chainId, validators);
    
    this.checkpoints.set(genesis.checkpointId, genesis);
    this.genesisCheckpoint = genesis;
    this.latestCheckpoint = genesis;
    this.checkpointHeight = 0;

    console.log(`Genesis checkpoint created: ${genesis.checkpointId}`);
    console.log(`Initial validators: ${validators.length}`);
    
    return genesis;
  }

  getGenesisCheckpoint(): Checkpoint | null {
    return this.genesisCheckpoint;
  }

  getGenesisConfig(): GenesisConfig | null {
    if (!this.genesisCheckpoint) return null;
    return {
      chainId: this.chainId,
      genesisTime: this.genesisCheckpoint.timestamp,
      initialValidators: this.genesisCheckpoint.validators,
      genesisCheckpointId: this.genesisCheckpoint.checkpointId
    };
  }

  async createCheckpoint(): Promise<Checkpoint> {
    this.checkpointHeight++;

    const previousCheckpointId = this.latestCheckpoint?.checkpointId || null;
    const validators = this.deps.getValidatorEntries();
    
    const txHashes = this.deps.getAllTransactionHashes?.() || [];
    const txMerkleRoot = txHashes.length > 0 ? await getTransactionMerkleRoot(txHashes) : undefined;
    
    const stateRoot = this.deps.getStateRoot ? await this.deps.getStateRoot() : undefined;
    const receiptRoot = this.deps.getReceiptRoot ? await this.deps.getReceiptRoot() : undefined;

    const checkpoint = await createCheckpoint(
      this.checkpointHeight,
      this.deps.getMerkleRoot(),
      this.deps.getTipCount(),
      this.deps.getTotalTransactions(),
      this.deps.getTotalWeight(),
      validators,
      previousCheckpointId,
      txMerkleRoot,
      txHashes.length > 0 ? txHashes : undefined,
      stateRoot,
      receiptRoot
    );

    this.checkpoints.set(checkpoint.checkpointId, checkpoint);
    this.latestCheckpoint = checkpoint;

    this.pruneOldCheckpoints();

    if (this.onCheckpointCallback) {
      this.onCheckpointCallback(checkpoint.checkpointId, checkpoint.height);
    }

    return checkpoint;
  }

  async signCheckpoint(
    checkpointId: string,
    privateKey: Uint8Array,
    publicKey: Uint8Array,
    validatorAddress: string
  ): Promise<ValidatorSignature | null> {
    const checkpoint = this.checkpoints.get(checkpointId);
    if (!checkpoint) return null;

    const validators = this.deps.getValidatorEntries();
    const validator = validators.find(v => v.address === validatorAddress);
    const validatorWeight = validator?.weight || 0;

    const baseSigningData = getCheckpointSigningData(checkpoint);
    const signerSigningData = baseSigningData + `:${validatorWeight}`;
    const signature = await sign(signerSigningData, privateKey);

    const validatorSig: ValidatorSignature = {
      validator: validatorAddress,
      signature,
      publicKey: Array.from(publicKey),
      weight: validatorWeight,
      timestamp: Date.now()
    };

    checkpoint.signatures.push(validatorSig);
    return validatorSig;
  }

  async addExternalSignature(
    checkpointId: string,
    signature: ValidatorSignature
  ): Promise<boolean> {
    const checkpoint = this.checkpoints.get(checkpointId);
    if (!checkpoint) return false;

    const validators = this.deps.getValidatorEntries();
    const knownValidator = validators.find(v => v.address === signature.validator);
    if (!knownValidator) {
      console.log(`Rejected signature: unknown validator ${signature.validator}`);
      return false;
    }

    if (Math.abs(knownValidator.weight - signature.weight) > 0.001) {
      console.log(`Rejected signature: weight mismatch for ${signature.validator} (claimed ${signature.weight}, actual ${knownValidator.weight})`);
      return false;
    }

    const baseSigningData = getCheckpointSigningData(checkpoint);
    const signerSigningData = baseSigningData + `:${signature.weight}`;
    const isValid = await verify(
      signerSigningData,
      signature.signature,
      new Uint8Array(signature.publicKey)
    );

    if (!isValid) return false;

    const alreadySigned = checkpoint.signatures.some(
      s => s.validator === signature.validator
    );
    if (alreadySigned) return false;

    checkpoint.signatures.push(signature);
    return true;
  }

  getCheckpoint(checkpointId: string): Checkpoint | undefined {
    return this.checkpoints.get(checkpointId);
  }

  getLatestCheckpoint(): Checkpoint | null {
    return this.latestCheckpoint;
  }

  async getTransactionMerkleProof(
    txHash: string,
    checkpointId: string
  ): Promise<{ proof: string[]; index: number; txMerkleRoot: string } | null> {
    const checkpoint = this.checkpoints.get(checkpointId);
    if (!checkpoint || !checkpoint.txMerkleRoot || !checkpoint.txHashes) return null;
    
    if (checkpoint.txHashes.length === 0) return null;
    
    const { getTransactionMerkleProof: getMerkleProof } = await import('@rinku/core');
    const proofResult = await getMerkleProof(checkpoint.txHashes, txHash);
    if (!proofResult) return null;
    
    return {
      proof: proofResult.proof,
      index: proofResult.index,
      txMerkleRoot: checkpoint.txMerkleRoot
    };
  }

  getCheckpointProof(checkpointId?: string): CheckpointProof | null {
    const checkpoint = checkpointId 
      ? this.checkpoints.get(checkpointId)
      : this.latestCheckpoint;

    if (!checkpoint) return null;

    const validators = this.deps.getValidatorEntries();
    const totalWeight = validators.reduce((sum, v) => sum + v.weight, 0);

    const signerWeight = checkpoint.signatures.reduce((sum, sig) => {
      const validator = validators.find(v => v.address === sig.validator);
      return sum + (validator?.weight || 0);
    }, 0);

    const weightPercent = totalWeight > 0 
      ? (signerWeight / totalWeight) * 100 
      : 0;

    return createCheckpointProof(checkpoint, weightPercent);
  }

  getCheckpointChain(): CheckpointProof[] {
    const chain: CheckpointProof[] = [];
    const allCheckpoints = this.getAllCheckpoints();
    
    for (const checkpoint of allCheckpoints) {
      const proof = this.getCheckpointProof(checkpoint.checkpointId);
      if (proof) chain.push(proof);
    }
    
    return chain.sort((a, b) => a.checkpointHeight - b.checkpointHeight);
  }

  isCheckpointFinalized(checkpointId: string): boolean {
    const checkpoint = this.checkpoints.get(checkpointId);
    if (!checkpoint) return false;

    if (checkpoint.signatures.length < this.config.minSignaturesRequired) {
      return false;
    }

    const proof = this.getCheckpointProof(checkpointId);
    if (!proof) return false;

    return proof.totalValidatorWeight >= this.config.minValidatorWeightPercent;
  }

  private pruneOldCheckpoints(): void {
    if (this.checkpoints.size <= this.maxCheckpoints) return;

    const sorted = Array.from(this.checkpoints.entries())
      .sort((a, b) => b[1].height - a[1].height);

    const toKeep = new Set<string>();
    if (this.genesisCheckpoint) {
      toKeep.add(this.genesisCheckpoint.checkpointId);
    }
    if (this.latestCheckpoint) {
      toKeep.add(this.latestCheckpoint.checkpointId);
    }

    let kept = 0;
    for (const [id] of sorted) {
      if (kept >= this.maxCheckpoints && !toKeep.has(id)) {
        this.checkpoints.delete(id);
      } else {
        kept++;
      }
    }
  }

  getAllCheckpoints(): Checkpoint[] {
    return Array.from(this.checkpoints.values())
      .sort((a, b) => b.height - a.height);
  }

  start(intervalMs?: number): void {
    if (this.intervalId) return;

    const interval = intervalMs || this.config.checkpointIntervalMs;

    this.intervalId = setInterval(async () => {
      try {
        const checkpoint = await this.createCheckpoint();
        console.log(`Created checkpoint ${checkpoint.checkpointId} at height ${checkpoint.height}`);

        const privateKey = this.deps.getPrivateKey();
        const nodeAddress = this.deps.getNodeAddress();
        const publicKey = this.deps.getPublicKey(nodeAddress);

        if (privateKey && publicKey) {
          await this.signCheckpoint(
            checkpoint.checkpointId,
            privateKey,
            publicKey,
            nodeAddress
          );
          console.log(`Signed checkpoint ${checkpoint.checkpointId}`);
        }
      } catch (err) {
        console.error('Failed to create checkpoint:', err);
      }
    }, interval);
  }

  stop(): void {
    if (this.intervalId) {
      clearInterval(this.intervalId);
      this.intervalId = null;
    }
  }

  toJSON(): object {
    return {
      checkpoints: Array.from(this.checkpoints.entries()),
      latestCheckpointId: this.latestCheckpoint?.checkpointId || null,
      genesisCheckpointId: this.genesisCheckpoint?.checkpointId || null,
      checkpointHeight: this.checkpointHeight,
      chainId: this.chainId,
      config: this.config
    };
  }

  static fromJSON(
    data: any,
    deps: CheckpointServiceDeps
  ): CheckpointService {
    const service = new CheckpointService(deps, data.chainId, data.config);

    if (data.checkpoints) {
      for (const [id, checkpoint] of data.checkpoints) {
        service.checkpoints.set(id, checkpoint);
      }
    }

    if (data.latestCheckpointId) {
      service.latestCheckpoint = service.checkpoints.get(data.latestCheckpointId) || null;
    }

    if (data.genesisCheckpointId) {
      service.genesisCheckpoint = service.checkpoints.get(data.genesisCheckpointId) || null;
    }

    service.checkpointHeight = data.checkpointHeight || 0;

    return service;
  }
}
