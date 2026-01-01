import { hash as cryptoHash } from './crypto.js';
import type { 
  ContractState, 
  ContractDeploy, 
  ContractCall, 
  StateDiff, 
  StateChange,
  ExecutionResult 
} from './types.js';

export function computeStateHash(state: Record<string, unknown>): string {
  const sorted = JSON.stringify(state, Object.keys(state).sort());
  const encoder = new TextEncoder();
  const bytes = encoder.encode(sorted);
  let h = 0;
  for (let i = 0; i < bytes.length; i++) {
    h = ((h << 5) - h) + bytes[i];
    h = h & h;
  }
  return Math.abs(h).toString(16).padStart(8, '0');
}

export async function computeStateHashAsync(state: Record<string, unknown>): Promise<string> {
  const sorted = JSON.stringify(state, Object.keys(state).sort());
  return cryptoHash(sorted);
}

export function createContractId(creator: string, nonce: number): string {
  const data = `${creator}:${nonce}`;
  let h = 0;
  for (let i = 0; i < data.length; i++) {
    h = ((h << 5) - h) + data.charCodeAt(i);
    h = h & h;
  }
  return `sc_${Math.abs(h).toString(16).padStart(8, '0')}`;
}

export function computeStateDiff(
  contractId: string,
  height: number,
  oldState: Record<string, unknown>,
  newState: Record<string, unknown>
): StateDiff {
  const changes: StateChange[] = [];
  const allKeys = new Set([...Object.keys(oldState), ...Object.keys(newState)]);
  
  for (const key of allKeys) {
    const oldValue = oldState[key];
    const newValue = newState[key];
    
    if (JSON.stringify(oldValue) !== JSON.stringify(newValue)) {
      changes.push({ key, oldValue, newValue });
    }
  }
  
  return {
    contractId,
    height,
    changes,
    preHash: computeStateHash(oldState),
    postHash: computeStateHash(newState)
  };
}

export function applyStateDiff(
  state: Record<string, unknown>,
  diff: StateDiff
): Record<string, unknown> {
  const newState = { ...state };
  
  for (const change of diff.changes) {
    if (change.newValue === undefined) {
      delete newState[change.key];
    } else {
      newState[change.key] = change.newValue;
    }
  }
  
  return newState;
}

export function createContractState(deploy: ContractDeploy, deployUrl: string): ContractState {
  return {
    contractId: deploy.contractId,
    creator: deploy.creator,
    wasmBase64: deploy.wasmBase64,
    deployUrl,
    state: deploy.initState,
    stateHash: computeStateHash(deploy.initState),
    height: 0,
    createdAt: deploy.ts
  };
}

export function validateContractCall(
  call: ContractCall,
  contractState: ContractState
): { valid: boolean; error?: string } {
  if (call.contractId !== contractState.contractId) {
    return { valid: false, error: 'Contract ID mismatch' };
  }
  
  if (call.preStateHash !== contractState.stateHash) {
    return { valid: false, error: 'Pre-state hash mismatch - state has changed' };
  }
  
  return { valid: true };
}

export interface WasmHostBindings {
  getBalance(address: string): number;
  getAccountAge(address: string): number;
  log(message: string): void;
  getCurrentTime(): number;
}

export function createMockRuntime(): {
  execute: (
    contractId: string,
    wasmBase64: string,
    entrypoint: string,
    input: Record<string, unknown>,
    state: Record<string, unknown>,
    height: number,
    bindings: WasmHostBindings
  ) => ExecutionResult;
} {
  return {
    execute: (contractId, wasmBase64, entrypoint, input, state, height, bindings) => {
      const logs: string[] = [];
      const startGas = 1000000;
      let gasUsed = 0;
      
      try {
        const newState = JSON.parse(JSON.stringify(state));
        
        if (entrypoint === 'init') {
          gasUsed = 1000;
          return {
            success: true,
            stateDiff: computeStateDiff(contractId, height, state, newState),
            gasUsed,
            logs
          };
        }
        
        if (entrypoint === 'transfer') {
          const { from, to, amount } = input as { from: string; to: string; amount: number };
          const balances = (newState.balances || {}) as Record<string, number>;
          
          const fromBalance = balances[from] || 0;
          if (fromBalance < amount) {
            return {
              success: false,
              stateDiff: null,
              gasUsed: 5000,
              error: 'Insufficient balance',
              logs
            };
          }
          
          balances[from] = fromBalance - amount;
          balances[to] = (balances[to] || 0) + amount;
          newState.balances = balances;
          
          gasUsed = 10000;
          logs.push(`Transferred ${amount} from ${from} to ${to}`);
        }
        
        if (entrypoint === 'mint') {
          const { to, amount } = input as { to: string; amount: number };
          const balances = (newState.balances || {}) as Record<string, number>;
          balances[to] = (balances[to] || 0) + amount;
          newState.balances = balances;
          
          gasUsed = 8000;
          logs.push(`Minted ${amount} to ${to}`);
        }
        
        if (entrypoint === 'get_balance') {
          const { address } = input as { address: string };
          const balances = (state.balances || {}) as Record<string, number>;
          gasUsed = 1000;
          logs.push(`Balance of ${address}: ${balances[address] || 0}`);
          return {
            success: true,
            stateDiff: null,
            gasUsed,
            logs
          };
        }
        
        return {
          success: true,
          stateDiff: computeStateDiff(contractId, height, state, newState),
          gasUsed,
          logs
        };
      } catch (error) {
        return {
          success: false,
          stateDiff: null,
          gasUsed: startGas,
          error: error instanceof Error ? error.message : 'Unknown error',
          logs
        };
      }
    }
  };
}

export const SUPPORTED_ENTRYPOINTS = ['init', 'transfer', 'mint', 'burn', 'get_balance', 'get_owner'] as const;
export type SupportedEntrypoint = typeof SUPPORTED_ENTRYPOINTS[number];

export const GAS_LIMITS = {
  deploy: 100000,
  call: 50000,
  query: 10000
} as const;
