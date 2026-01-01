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
  verify
} from '@rinku/core';

export interface CheckpointServiceDeps {
  getMerkleRoot: () => string;
  getTipUrls: () => string[];
  getTotalTransactions: () => number;
  getValidatorEntries: () => ValidatorEntry[];
  getTotalWeight: () => number;
  getPublicKey: (address: string) => Uint8Array | undefined;
  getPrivateKey: () => Uint8Array | undefined;
  getNodeAddress: () => string;
}

export class CheckpointService {
  private checkpoints: Map<string, Checkpoint> = new Map();
  private latestCheckpoint: Checkpoint | null = null;
  private genesisCheckpoint: Checkpoint | null = null;
  private checkpointHeight = 0;
  private config: CheckpointConfig;
  private intervalId: NodeJS.Timeout | null = null;
  private chainId: string;

  constructor(
    private deps: CheckpointServiceDeps,
    chainId: string = 'rinku-testnet',
    config?: Partial<CheckpointConfig>
  ) {
    this.config = { ...DEFAULT_CHECKPOINT_CONFIG, ...config };
    this.chainId = chainId;
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

    const checkpoint = await createCheckpoint(
      this.checkpointHeight,
      this.deps.getMerkleRoot(),
      this.deps.getTipUrls(),
      this.deps.getTotalTransactions(),
      this.deps.getTotalWeight(),
      validators,
      previousCheckpointId
    );

    this.checkpoints.set(checkpoint.checkpointId, checkpoint);
    this.latestCheckpoint = checkpoint;

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
