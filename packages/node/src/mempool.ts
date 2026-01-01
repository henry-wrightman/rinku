import type { SignedTransaction } from '@rinku/core';

export class Mempool {
  private pending: Map<string, SignedTransaction> = new Map();
  private maxSize: number;

  constructor(maxSize: number = 1000) {
    this.maxSize = maxSize;
  }

  add(tx: SignedTransaction): boolean {
    if (this.pending.size >= this.maxSize) {
      return false;
    }

    if (this.pending.has(tx.hash)) {
      return false;
    }

    this.pending.set(tx.hash, tx);
    return true;
  }

  remove(hash: string): boolean {
    return this.pending.delete(hash);
  }

  get(hash: string): SignedTransaction | undefined {
    return this.pending.get(hash);
  }

  getAll(): SignedTransaction[] {
    return Array.from(this.pending.values());
  }

  has(hash: string): boolean {
    return this.pending.has(hash);
  }

  size(): number {
    return this.pending.size;
  }

  clear(): void {
    this.pending.clear();
  }

  getByAccount(fingerprint: string): SignedTransaction[] {
    return this.getAll().filter(tx => tx.from === fingerprint);
  }
}
