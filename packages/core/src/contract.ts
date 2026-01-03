import { hash as cryptoHash } from './crypto.js';
import type { 
  ContractState, 
  ContractDeploy, 
  ContractCall, 
  StateDiff, 
  StateChange,
  ExecutionResult 
} from './types.js';

export interface GasSchedule {
  baseExecution: number;
  storageRead: number;
  storageWrite: number;
  storageDelete: number;
  memoryAlloc: number;
  log: number;
  emit: number;
  hash: number;
  balanceCheck: number;
  accountAgeCheck: number;
  transfer: number;
  mint: number;
  burn: number;
}

export const DEFAULT_GAS_SCHEDULE: GasSchedule = {
  baseExecution: 1000,
  storageRead: 200,
  storageWrite: 5000,
  storageDelete: 5000,
  memoryAlloc: 3,
  log: 100,
  emit: 500,
  hash: 300,
  balanceCheck: 100,
  accountAgeCheck: 100,
  transfer: 8000,
  mint: 6000,
  burn: 6000
};

export class GasMeter {
  private gasUsed: number = 0;
  private readonly gasLimit: number;
  private readonly schedule: GasSchedule;

  constructor(gasLimit: number, schedule: GasSchedule = DEFAULT_GAS_SCHEDULE) {
    this.gasLimit = gasLimit;
    this.schedule = schedule;
  }

  charge(operation: keyof GasSchedule, multiplier: number = 1): boolean {
    const cost = this.schedule[operation] * multiplier;
    this.gasUsed += cost;
    return this.gasUsed <= this.gasLimit;
  }

  chargeCustom(amount: number): boolean {
    this.gasUsed += amount;
    return this.gasUsed <= this.gasLimit;
  }

  getGasUsed(): number {
    return this.gasUsed;
  }

  getGasRemaining(): number {
    return Math.max(0, this.gasLimit - this.gasUsed);
  }

  isOutOfGas(): boolean {
    return this.gasUsed > this.gasLimit;
  }

  getSchedule(): GasSchedule {
    return { ...this.schedule };
  }
}

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
  emit(eventName: string, data: Record<string, unknown>): void;
  getCurrentTime(): number;
  getBlockHeight(): number;
}

export interface WasmRuntimeConfig {
  maxGas: number;
  maxMemoryPages: number;
  maxExecutionTimeMs: number;
  allowedHostFunctions: string[];
}

export const DEFAULT_RUNTIME_CONFIG: WasmRuntimeConfig = {
  maxGas: 1_000_000,
  maxMemoryPages: 256,
  maxExecutionTimeMs: 5000,
  allowedHostFunctions: [
    'getBalance',
    'getAccountAge',
    'log',
    'emit',
    'getCurrentTime',
    'getBlockHeight',
    'storage_read',
    'storage_write'
  ]
};

export interface WasmExecutionContext {
  contractId: string;
  caller: string;
  gasLimit: number;
  blockHeight: number;
  timestamp: number;
}

export interface WasmRuntimeInterface {
  execute(
    wasmCode: Uint8Array,
    entrypoint: string,
    input: Record<string, unknown>,
    state: Record<string, unknown>,
    context: WasmExecutionContext,
    bindings: WasmHostBindings
  ): Promise<ExecutionResult>;
  
  validate(wasmCode: Uint8Array): Promise<{ valid: boolean; error?: string }>;
}

export interface EmittedEvent {
  eventName: string;
  data: Record<string, unknown>;
}

export interface ExtendedExecutionResult extends ExecutionResult {
  events: EmittedEvent[];
}

export function createMockRuntime(
  gasLimit: number = 1_000_000,
  schedule: GasSchedule = DEFAULT_GAS_SCHEDULE
): {
  execute: (
    contractId: string,
    wasmBase64: string,
    entrypoint: string,
    input: Record<string, unknown>,
    state: Record<string, unknown>,
    height: number,
    bindings: WasmHostBindings
  ) => ExtendedExecutionResult;
  getGasSchedule: () => GasSchedule;
} {
  return {
    getGasSchedule: () => ({ ...schedule }),
    execute: (contractId, wasmBase64, entrypoint, input, state, height, bindings) => {
      const logs: string[] = [];
      const events: EmittedEvent[] = [];
      const meter = new GasMeter(gasLimit, schedule);
      
      meter.charge('baseExecution');
      
      const wrappedBindings: WasmHostBindings = {
        ...bindings,
        log: (message: string) => {
          meter.charge('log');
          logs.push(message);
          bindings.log(message);
        },
        emit: (eventName: string, data: Record<string, unknown>) => {
          meter.charge('emit');
          events.push({ eventName, data });
          bindings.emit(eventName, data);
        },
        getBalance: (address: string) => {
          meter.charge('balanceCheck');
          return bindings.getBalance(address);
        },
        getAccountAge: (address: string) => {
          meter.charge('accountAgeCheck');
          return bindings.getAccountAge(address);
        }
      };
      
      try {
        const newState = JSON.parse(JSON.stringify(state));
        meter.charge('storageRead');
        
        if (meter.isOutOfGas()) {
          return {
            success: false,
            stateDiff: null,
            gasUsed: meter.getGasUsed(),
            error: 'Out of gas during initialization',
            logs,
            events
          };
        }
        
        if (entrypoint === 'init') {
          events.push({ eventName: 'Initialized', data: { contractId } });
          meter.charge('emit');
          return {
            success: true,
            stateDiff: computeStateDiff(contractId, height, state, newState),
            gasUsed: meter.getGasUsed(),
            logs,
            events
          };
        }
        
        if (entrypoint === 'transfer') {
          const { from, to, amount } = input as { from: string; to: string; amount: number };
          const balances = (newState.balances || {}) as Record<string, number>;
          
          meter.charge('balanceCheck');
          const fromBalance = balances[from] || 0;
          
          if (fromBalance < amount) {
            meter.charge('emit');
            events.push({ eventName: 'TransferFailed', data: { from, to, amount, reason: 'Insufficient balance' } });
            return {
              success: false,
              stateDiff: null,
              gasUsed: meter.getGasUsed(),
              error: 'Insufficient balance',
              logs,
              events
            };
          }
          
          meter.charge('transfer');
          meter.charge('storageWrite', 2);
          
          if (meter.isOutOfGas()) {
            return {
              success: false,
              stateDiff: null,
              gasUsed: meter.getGasUsed(),
              error: 'Out of gas during transfer',
              logs,
              events
            };
          }
          
          balances[from] = fromBalance - amount;
          balances[to] = (balances[to] || 0) + amount;
          newState.balances = balances;
          
          meter.charge('log');
          logs.push(`Transferred ${amount} from ${from} to ${to}`);
          meter.charge('emit');
          events.push({ eventName: 'Transfer', data: { from, to, amount } });
        }
        
        if (entrypoint === 'mint') {
          const { to, amount } = input as { to: string; amount: number };
          const balances = (newState.balances || {}) as Record<string, number>;
          
          meter.charge('mint');
          meter.charge('storageWrite');
          
          if (meter.isOutOfGas()) {
            return {
              success: false,
              stateDiff: null,
              gasUsed: meter.getGasUsed(),
              error: 'Out of gas during mint',
              logs,
              events
            };
          }
          
          balances[to] = (balances[to] || 0) + amount;
          newState.balances = balances;
          
          meter.charge('log');
          logs.push(`Minted ${amount} to ${to}`);
          meter.charge('emit');
          events.push({ eventName: 'Mint', data: { to, amount } });
        }
        
        if (entrypoint === 'burn') {
          const { from, amount } = input as { from: string; amount: number };
          const balances = (newState.balances || {}) as Record<string, number>;
          
          meter.charge('balanceCheck');
          const fromBalance = balances[from] || 0;
          
          if (fromBalance < amount) {
            meter.charge('emit');
            events.push({ eventName: 'BurnFailed', data: { from, amount, reason: 'Insufficient balance' } });
            return {
              success: false,
              stateDiff: null,
              gasUsed: meter.getGasUsed(),
              error: 'Insufficient balance for burn',
              logs,
              events
            };
          }
          
          meter.charge('burn');
          meter.charge('storageWrite');
          
          if (meter.isOutOfGas()) {
            return {
              success: false,
              stateDiff: null,
              gasUsed: meter.getGasUsed(),
              error: 'Out of gas during burn',
              logs,
              events
            };
          }
          
          balances[from] = fromBalance - amount;
          newState.balances = balances;
          
          meter.charge('log');
          logs.push(`Burned ${amount} from ${from}`);
          meter.charge('emit');
          events.push({ eventName: 'Burn', data: { from, amount } });
        }
        
        if (entrypoint === 'get_balance') {
          const { address } = input as { address: string };
          const balances = (state.balances || {}) as Record<string, number>;
          meter.charge('storageRead');
          meter.charge('log');
          logs.push(`Balance of ${address}: ${balances[address] || 0}`);
          return {
            success: true,
            stateDiff: null,
            gasUsed: meter.getGasUsed(),
            logs,
            events
          };
        }
        
        if (meter.isOutOfGas()) {
          return {
            success: false,
            stateDiff: null,
            gasUsed: meter.getGasUsed(),
            error: 'Out of gas',
            logs,
            events
          };
        }
        
        return {
          success: true,
          stateDiff: computeStateDiff(contractId, height, state, newState),
          gasUsed: meter.getGasUsed(),
          logs,
          events
        };
      } catch (error) {
        return {
          success: false,
          stateDiff: null,
          gasUsed: meter.getGasUsed(),
          error: error instanceof Error ? error.message : 'Unknown error',
          logs,
          events
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
