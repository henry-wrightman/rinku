import { getMerkleRoot, type AccountState, type SignedTransaction } from '@rinku/core';

export class StateManager {
  private accounts: Map<string, AccountState> = new Map();
  private merkleRoot: string = '';

  constructor() {
    this.accounts = new Map();
  }

  getAccount(fingerprint: string): AccountState | undefined {
    return this.accounts.get(fingerprint);
  }

  getAllAccounts(): Map<string, AccountState> {
    return new Map(this.accounts);
  }

  createAccount(fingerprint: string, initialBalance: number = 0): AccountState {
    const account: AccountState = {
      fingerprint,
      balance: initialBalance,
      nonce: 0,
      firstTxTimestamp: Date.now()
    };

    this.accounts.set(fingerprint, account);
    return account;
  }

  async applyTransaction(tx: SignedTransaction, opts?: { skipChecks?: boolean }): Promise<boolean> {
    const skipChecks = opts?.skipChecks || false;
    const sender = this.accounts.get(tx.from);
    
    if (!sender && tx.from !== 'genesis' && tx.from !== 'faucet') {
      if (!skipChecks) return false;
    }

    if (sender && !skipChecks) {
      if (sender.balance < tx.amount) {
        return false;
      }

      if (tx.nonce !== sender.nonce + 1) {
        return false;
      }
    }

    if (sender) {
      sender.balance -= tx.amount;
      sender.nonce = tx.nonce;
    }

    let receiver = this.accounts.get(tx.to);
    if (!receiver) {
      receiver = this.createAccount(tx.to, 0);
    }
    receiver.balance += tx.amount;

    await this.updateMerkleRoot();

    return true;
  }

  async updateMerkleRoot(): Promise<string> {
    this.merkleRoot = await getMerkleRoot(this.accounts);
    return this.merkleRoot;
  }

  getMerkleRoot(): string {
    return this.merkleRoot;
  }

  setFaucetAccount(fingerprint: string, balance: number): void {
    const account: AccountState = {
      fingerprint,
      balance,
      nonce: 0,
      firstTxTimestamp: Date.now()
    };
    this.accounts.set(fingerprint, account);
  }

  toJSON(): object {
    return {
      accounts: Array.from(this.accounts.entries()),
      merkleRoot: this.merkleRoot
    };
  }

  static fromJSON(data: any): StateManager {
    const state = new StateManager();
    state.accounts = new Map(data.accounts);
    state.merkleRoot = data.merkleRoot;
    return state;
  }
}
