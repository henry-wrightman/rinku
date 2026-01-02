import { KeyManager } from './keys.js';
import { createTransaction, createAndSignTransaction, createURL } from './tx.js';
import type { KeyPair, SignedTransaction, AccountState } from '@rinku/core';

export interface WalletState {
  fingerprint: string;
  balance: number;
  nonce: number;
}

export class Wallet {
  private keyManager: KeyManager;
  private nodeUrl: string;
  private state: WalletState | null = null;

  constructor(nodeUrl: string = 'http://localhost:3001') {
    this.keyManager = new KeyManager();
    this.nodeUrl = nodeUrl;
  }

  async create(): Promise<string> {
    await this.keyManager.create();
    return this.keyManager.getFingerprint();
  }

  async import(serialized: string): Promise<string> {
    await this.keyManager.import(serialized);
    return this.keyManager.getFingerprint();
  }

  export(): string {
    return this.keyManager.export();
  }

  getFingerprint(): string {
    return this.keyManager.getFingerprint();
  }

  getPublicKey(): Uint8Array {
    return this.keyManager.getPublicKey();
  }

  async refresh(): Promise<WalletState> {
    const fingerprint = this.keyManager.getFingerprint();
    
    try {
      const response = await fetch(`${this.nodeUrl}/api/account/${fingerprint}`);
      if (response.ok) {
        const data = await response.json() as { balance: number; nonce: number };
        this.state = {
          fingerprint,
          balance: data.balance,
          nonce: data.nonce
        };
      } else {
        this.state = {
          fingerprint,
          balance: 0,
          nonce: 0
        };
      }
    } catch {
      this.state = {
        fingerprint,
        balance: 0,
        nonce: 0
      };
    }

    return this.state;
  }

  async getBalance(): Promise<number> {
    const state = await this.refresh();
    return state.balance;
  }

  async getGasPrice(): Promise<number> {
    try {
      const response = await fetch(`${this.nodeUrl}/api/gas/price`);
      if (response.ok) {
        const data = await response.json() as { currentPrice: number };
        return data.currentPrice;
      }
    } catch {}
    return 0.01; // fallback default
  }

  async send(to: string, amount: number, fee?: number): Promise<{ tx: SignedTransaction; url: string }> {
    await this.refresh();
    
    if (!this.state) {
      throw new Error('Wallet not initialized');
    }

    const actualFee = fee ?? 0.01;
    if (this.state.balance < amount + actualFee) {
      throw new Error('Insufficient balance for amount + fee');
    }

    const tipsResponse = await fetch(`${this.nodeUrl}/api/tipUrls`);
    const tipsData = await tipsResponse.json() as { tipUrls: string[] };
    let tipUrls = tipsData.tipUrls.slice(0, 2);

    if (tipUrls.length === 0) {
      tipUrls = [];
    }

    const { tx, url } = await createAndSignTransaction(
      this.keyManager.getKeyPair(),
      {
        to,
        amount,
        fee: actualFee,
        nonce: this.state.nonce + 1,
        tipUrls
      }
    );

    const submitResponse = await fetch(`${this.nodeUrl}/api/tx`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        tx,
        publicKey: Array.from(this.keyManager.getPublicKey())
      })
    });

    if (!submitResponse.ok) {
      const error = await submitResponse.json() as { error?: string };
      throw new Error(error.error || 'Transaction failed');
    }

    return { tx, url: url.path };
  }
}

export { KeyManager } from './keys.js';
export { createTransaction, createAndSignTransaction, createURL } from './tx.js';
