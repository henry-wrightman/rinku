import type { AccountState, Weight } from './types.js';

const AGE_WEIGHT = 0.3;
const BALANCE_WEIGHT = 0.7;

export function calculateWeight(account: AccountState): Weight {
  const now = Date.now();
  const ageMs = now - account.firstTxTimestamp;
  const ageDays = ageMs / (1000 * 60 * 60 * 24);

  const accountAge = ageDays * AGE_WEIGHT;
  const balance = account.balance * BALANCE_WEIGHT;
  const total = accountAge + balance;

  return {
    accountAge,
    balance,
    total
  };
}

export function calculateAccountWeights(accounts: Map<string, AccountState>): Map<string, number> {
  const weights = new Map<string, number>();

  for (const [fingerprint, account] of accounts) {
    const weight = calculateWeight(account);
    weights.set(fingerprint, weight.total);
  }

  return weights;
}

export function normalizeWeights(weights: Map<string, number>): Map<string, number> {
  const total = Array.from(weights.values()).reduce((sum, w) => sum + w, 0);
  
  if (total === 0) return new Map(weights);

  const normalized = new Map<string, number>();
  for (const [key, value] of weights) {
    normalized.set(key, value / total);
  }

  return normalized;
}
