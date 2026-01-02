import {
  sign,
  hashTransaction,
  createTransactionURL,
  type Transaction,
  type SignedTransaction,
  type TransactionURL,
  type KeyPair
} from '@rinku/core';

export interface CreateTransactionOptions {
  to: string;
  amount: number;
  nonce: number;
  tipUrls: string[];
  fee?: number;
}

const DEFAULT_FEE = 0.01;

export async function createTransaction(
  keyPair: KeyPair,
  options: CreateTransactionOptions
): Promise<SignedTransaction> {
  const tx: Transaction = {
    from: keyPair.fingerprint,
    to: options.to,
    amount: options.amount,
    fee: options.fee ?? DEFAULT_FEE,
    nonce: options.nonce,
    tipUrls: options.tipUrls,
    sig: '',
    ts: Date.now()
  };

  const txHash = await hashTransaction(tx);

  const signature = await sign(txHash, keyPair.privateKey);
  tx.sig = signature;

  return {
    ...tx,
    hash: txHash
  };
}

export function createURL(tx: Transaction): TransactionURL {
  return createTransactionURL(tx);
}

export async function createAndSignTransaction(
  keyPair: KeyPair,
  options: CreateTransactionOptions
): Promise<{ tx: SignedTransaction; url: TransactionURL }> {
  const tx = await createTransaction(keyPair, options);
  const url = createURL(tx);
  return { tx, url };
}
