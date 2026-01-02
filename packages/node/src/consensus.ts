import {
  DAG,
  verify,
  hashTransaction,
  calculateAccountWeights,
  type SignedTransaction,
  type AccountState,
  type SelfCrawlableBundle
} from '@rinku/core';

export interface ValidationResult {
  valid: boolean;
  error?: string;
}

export class Consensus {
  private dag: DAG;
  private publicKeys: Map<string, Uint8Array> = new Map();

  constructor() {
    this.dag = new DAG();
  }

  registerPublicKey(fingerprint: string, publicKey: Uint8Array): void {
    this.publicKeys.set(fingerprint, publicKey);
  }

  async validateTransaction(
    tx: SignedTransaction,
    accounts: Map<string, AccountState>,
    publicKey?: Uint8Array
  ): Promise<ValidationResult> {
    if (tx.amount <= 0) {
      return { valid: false, error: 'Amount must be positive' };
    }

    if (!tx.from || !tx.to) {
      return { valid: false, error: 'Invalid addresses' };
    }

    const dagTips = this.dag.getTips();
    if (tx.tipUrls.length < 1 && dagTips.length > 0 && tx.from !== 'genesis') {
      return { valid: false, error: 'Must reference at least one tip' };
    }

    for (const tipUrl of tx.tipUrls) {
      const tipHash = this.dag.resolveUrlToHash(tipUrl);
      if (!tipHash || !this.dag.getNode(tipHash)) {
        return { valid: false, error: `Invalid tip URL reference: ${tipUrl.slice(0, 30)}...` };
      }
    }

    const sender = accounts.get(tx.from);
    if (tx.from !== 'faucet' && tx.from !== 'genesis') {
      if (!sender) {
        return { valid: false, error: 'Sender account not found' };
      }

      if (sender.balance < tx.amount) {
        return { valid: false, error: 'Insufficient balance' };
      }

      if (tx.nonce !== sender.nonce + 1) {
        return { valid: false, error: 'Invalid nonce' };
      }
    }

    const key = publicKey || this.publicKeys.get(tx.from);
    if (key && tx.from !== 'faucet' && tx.from !== 'genesis') {
      const txHash = await hashTransaction(tx);
      const isValid = await verify(txHash, tx.sig, key);
      if (!isValid) {
        return { valid: false, error: 'Invalid signature' };
      }
    }

    return { valid: true };
  }

  async addTransaction(tx: SignedTransaction): Promise<void> {
    await this.dag.addTransaction(tx);
  }

  selectTips(count: number = 2): string[] {
    const tips = this.dag.selectTips(count);
    return tips.length > 0 ? tips : [];
  }

  selectTipUrls(count: number = 2): string[] {
    const tipUrls = this.dag.selectTipUrls(count);
    return tipUrls;
  }

  updateWeights(accounts: Map<string, AccountState>): void {
    const weights = calculateAccountWeights(accounts);
    this.dag.updateWeights(weights);
  }

  resolveConflict(tx1Hash: string, tx2Hash: string): string {
    return this.dag.resolveConflict(tx1Hash, tx2Hash);
  }

  getDAG(): DAG {
    return this.dag;
  }

  getTips(): string[] {
    return this.dag.getTips();
  }

  getTipUrls(): string[] {
    return this.dag.getTipUrls();
  }

  getNode(hash: string) {
    return this.dag.getNode(hash);
  }

  getNodeByUrl(url: string) {
    const hash = this.dag.resolveUrlToHash(url);
    if (!hash) return undefined;
    return this.dag.getNode(hash);
  }

  getAllNodes() {
    return this.dag.getAllNodes();
  }

  hasTransaction(hash: string): boolean {
    return this.dag.getNode(hash) !== undefined;
  }

  getDAGSize(): number {
    return this.dag.size();
  }

  pruneDAG(maxNodes: number): number {
    return this.dag.pruneOldNodes(maxNodes);
  }

  getPublicKeys(): Map<string, Uint8Array> {
    return new Map(this.publicKeys);
  }

  getSelfCrawlableBundle(
    hash: string,
    getCheckpoint?: (checkpointId: string) => { checkpointId: string; merkleRoot: string; height: number; signatureCount: number } | null
  ): SelfCrawlableBundle | null {
    return this.dag.buildSelfCrawlableBundle(hash, getCheckpoint);
  }

  getSelfCrawlableUrl(
    hash: string,
    getCheckpoint?: (checkpointId: string) => { checkpointId: string; merkleRoot: string; height: number; signatureCount: number } | null
  ): string | null {
    return this.dag.getSelfCrawlableUrl(hash, getCheckpoint);
  }

  stampFinalityForAll(checkpointId: string, checkpointHeight: number): number {
    return this.dag.stampFinalityForAll(checkpointId, checkpointHeight);
  }

  hasFinality(hash: string): boolean {
    return this.dag.hasFinality(hash);
  }

  toJSON(): { dag: object; publicKeys: [string, number[]][] } {
    return {
      dag: this.dag.toJSON(),
      publicKeys: Array.from(this.publicKeys.entries()).map(([k, v]) => [k, Array.from(v)])
    };
  }

  static fromJSON(data: { dag: any; publicKeys: [string, number[]][] }): Consensus {
    const consensus = new Consensus();
    consensus.dag = DAG.fromJSON(data.dag);
    consensus.publicKeys = new Map(
      data.publicKeys.map(([k, v]) => [k, new Uint8Array(v)])
    );
    return consensus;
  }
}
