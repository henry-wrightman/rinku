import {
  decodeTransaction,
  parseTransactionURL,
  hashTransaction,
  type SignedTransaction,
  type AccountState,
  type Transaction
} from '@rinku/core';

export interface ValidationResult {
  valid: boolean;
  accounts: Map<string, AccountState>;
  transactions: Map<string, SignedTransaction>;
  tips: string[];
  tipUrls: string[];
  errors: string[];
  stats: {
    totalTransactions: number;
    totalAccounts: number;
    crawledUrls: number;
    failedUrls: number;
  };
}

export interface CrawlOptions {
  maxDepth?: number;
  timeout?: number;
  baseUrl?: string;
}

export class StatelessValidator {
  private visited: Set<string> = new Set();
  private transactions: Map<string, SignedTransaction> = new Map();
  private accounts: Map<string, AccountState> = new Map();
  private errors: string[] = [];
  private failedUrls: number = 0;
  
  async validateFromUrls(urls: string[], options: CrawlOptions = {}): Promise<ValidationResult> {
    this.visited.clear();
    this.transactions.clear();
    this.accounts.clear();
    this.errors = [];
    this.failedUrls = 0;

    for (const url of urls) {
      await this.crawlAndValidate(url, options, 0);
    }

    const sortedTxs = this.topologicalSort();
    
    for (const hash of sortedTxs) {
      const tx = this.transactions.get(hash)!;
      this.applyTransaction(tx);
    }

    const { tips, tipUrls } = this.findTips();

    return {
      valid: this.errors.length === 0,
      accounts: this.accounts,
      transactions: this.transactions,
      tips,
      tipUrls,
      errors: this.errors,
      stats: {
        totalTransactions: this.transactions.size,
        totalAccounts: this.accounts.size,
        crawledUrls: this.visited.size,
        failedUrls: this.failedUrls
      }
    };
  }

  private async crawlAndValidate(
    url: string, 
    options: CrawlOptions,
    depth: number
  ): Promise<void> {
    if (this.visited.has(url)) return;
    if (options.maxDepth && depth > options.maxDepth) return;

    this.visited.add(url);

    try {
      const tx = await this.fetchTransaction(url, options);
      if (!tx) {
        this.errors.push(`Failed to decode transaction from: ${url}`);
        this.failedUrls++;
        return;
      }

      const hash = await hashTransaction(tx);
      const signedTx: SignedTransaction = { ...tx, hash };
      
      this.transactions.set(hash, signedTx);

      for (const tipUrl of tx.tipUrls) {
        if (tipUrl.startsWith('/tx/') || tipUrl.startsWith('http')) {
          await this.crawlAndValidate(tipUrl, options, depth + 1);
        }
      }
    } catch (err: any) {
      this.errors.push(`Error processing ${url}: ${err.message}`);
      this.failedUrls++;
    }
  }

  private async fetchTransaction(
    url: string, 
    options: CrawlOptions
  ): Promise<Transaction | null> {
    if (url.startsWith('/tx/')) {
      return parseTransactionURL(url);
    }

    if (url.startsWith('http')) {
      try {
        const controller = new AbortController();
        const timeout = setTimeout(
          () => controller.abort(), 
          options.timeout || 10000
        );

        const response = await fetch(url, { 
          signal: controller.signal 
        });
        clearTimeout(timeout);

        if (!response.ok) {
          return null;
        }

        const data = await response.json() as { tx?: Transaction };
        return data.tx || parseTransactionURL(url);
      } catch {
        return null;
      }
    }

    return null;
  }

  private topologicalSort(): string[] {
    const visited = new Set<string>();
    const result: string[] = [];

    const visit = (hash: string) => {
      if (visited.has(hash)) return;
      visited.add(hash);

      const tx = this.transactions.get(hash);
      if (tx) {
        for (const tipUrl of tx.tipUrls) {
          const tipTx = parseTransactionURL(tipUrl);
          if (tipTx) {
            for (const [h, t] of this.transactions) {
              if (t.from === tipTx.from && t.nonce === tipTx.nonce && t.ts === tipTx.ts) {
                visit(h);
                break;
              }
            }
          }
        }
      }

      result.push(hash);
    };

    for (const hash of this.transactions.keys()) {
      visit(hash);
    }

    return result;
  }

  private applyTransaction(tx: SignedTransaction): void {
    const isGenesis = tx.from === 'genesis';
    const isFaucet = tx.from === 'faucet';

    if (!isGenesis && !isFaucet) {
      const fromAccount = this.accounts.get(tx.from);
      if (!fromAccount) {
        this.errors.push(`Missing sender account for tx ${tx.hash.slice(0, 8)}`);
        return;
      }
      if (fromAccount.balance < tx.amount) {
        this.errors.push(`Insufficient balance for tx ${tx.hash.slice(0, 8)}`);
        return;
      }
      fromAccount.balance -= tx.amount;
      fromAccount.nonce++;
    }

    let toAccount = this.accounts.get(tx.to);
    if (!toAccount) {
      toAccount = {
        fingerprint: tx.to,
        balance: 0,
        nonce: 0,
        firstTxTimestamp: tx.ts
      };
      this.accounts.set(tx.to, toAccount);
    }
    toAccount.balance += tx.amount;
  }

  private findTips(): { tips: string[]; tipUrls: string[] } {
    const referenced = new Set<string>();
    
    for (const tx of this.transactions.values()) {
      for (const tipUrl of tx.tipUrls) {
        const tipTx = parseTransactionURL(tipUrl);
        if (tipTx) {
          for (const [hash, t] of this.transactions) {
            if (t.from === tipTx.from && t.nonce === tipTx.nonce && t.ts === tipTx.ts) {
              referenced.add(hash);
              break;
            }
          }
        }
      }
    }

    const tips: string[] = [];
    const tipUrls: string[] = [];
    
    for (const [hash, tx] of this.transactions) {
      if (!referenced.has(hash)) {
        tips.push(hash);
        for (const url of this.visited) {
          if (url.startsWith('/tx/')) {
            const parsed = parseTransactionURL(url);
            if (parsed && parsed.from === tx.from && parsed.nonce === tx.nonce && parsed.ts === tx.ts) {
              tipUrls.push(url);
              break;
            }
          }
        }
      }
    }

    return { tips, tipUrls };
  }

  static async validateUrls(urls: string[], options?: CrawlOptions): Promise<ValidationResult> {
    const validator = new StatelessValidator();
    return validator.validateFromUrls(urls, options);
  }
}

export async function validateLedgerFromUrl(
  genesisUrl: string, 
  options?: CrawlOptions
): Promise<ValidationResult> {
  return StatelessValidator.validateUrls([genesisUrl], options);
}

export function extractUrlsFromText(text: string): string[] {
  const urlPattern = /\/tx\/[A-Za-z0-9_-]+/g;
  const matches = text.match(urlPattern) || [];
  return [...new Set(matches)];
}
