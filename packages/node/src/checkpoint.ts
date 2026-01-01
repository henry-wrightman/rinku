import {
  type Checkpoint,
  type CheckpointProof,
  type ValidatorSignature,
  type CheckpointConfig,
  type AccountState,
  type DAGNode,
  createCheckpoint,
  createCheckpointProof,
  getCheckpointSigningData,
  DEFAULT_CHECKPOINT_CONFIG,
  sign,
  verify
} from '@rinku/core';

export interface CheckpointServiceDeps {
  getMerkleRoot: () => string;
  getTipUrls: () => string[];
  getTotalTransactions: () => number;
  getValidators: () => { address: string; weight: number }[];
  getTotalWeight: () => number;
  getPublicKey: (address: string) => Uint8Array | undefined;
  getPrivateKey: () => Uint8Array | undefined;
  getNodeAddress: () => string;
}

export class CheckpointService {
  private checkpoints: Map<string, Checkpoint> = new Map();
  private latestCheckpoint: Checkpoint | null = null;
  private checkpointHeight = 0;
  private config: CheckpointConfig;
  private intervalId: NodeJS.Timeout | null = null;

  constructor(
    private deps: CheckpointServiceDeps,
    config?: Partial<CheckpointConfig>
  ) {
    this.config = { ...DEFAULT_CHECKPOINT_CONFIG, ...config };
  }

  getConfig(): CheckpointConfig {
    return { ...this.config };
  }

  async createCheckpoint(): Promise<Checkpoint> {
    this.checkpointHeight++;

    const checkpoint = await createCheckpoint(
      this.checkpointHeight,
      this.deps.getMerkleRoot(),
      this.deps.getTipUrls(),
      this.deps.getTotalTransactions(),
      this.deps.getTotalWeight()
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

    const signingData = getCheckpointSigningData(checkpoint);
    const signature = await sign(signingData, privateKey);

    const validatorSig: ValidatorSignature = {
      validator: validatorAddress,
      signature,
      publicKey: Array.from(publicKey),
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

    const signingData = getCheckpointSigningData(checkpoint);
    const isValid = await verify(
      signingData,
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

    const validators = this.deps.getValidators();
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
      checkpointHeight: this.checkpointHeight,
      config: this.config
    };
  }

  static fromJSON(
    data: any,
    deps: CheckpointServiceDeps
  ): CheckpointService {
    const service = new CheckpointService(deps, data.config);

    if (data.checkpoints) {
      for (const [id, checkpoint] of data.checkpoints) {
        service.checkpoints.set(id, checkpoint);
      }
    }

    if (data.latestCheckpointId) {
      service.latestCheckpoint = service.checkpoints.get(data.latestCheckpointId) || null;
    }

    service.checkpointHeight = data.checkpointHeight || 0;

    return service;
  }
}
