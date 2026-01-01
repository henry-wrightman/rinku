import {
  DAG,
  verify,
  hashTransaction,
  calculateAccountWeights,
  type SignedTransaction,
  type AccountState
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
    if (tx.tips.length < 1 && dagTips.length > 0 && tx.from !== 'genesis') {
      return { valid: false, error: 'Must reference at least one tip' };
    }

    for (const tip of tx.tips) {
      if (!this.dag.getNode(tip)) {
        return { valid: false, error: `Invalid tip reference: ${tip}` };
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
    return tips.length > 0 ? tips : ['genesis'];
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

  getNode(hash: string) {
    return this.dag.getNode(hash);
  }

  getAllNodes() {
    return this.dag.getAllNodes();
  }
}
