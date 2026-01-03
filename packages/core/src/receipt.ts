import { hash as cryptoHash } from './crypto.js';
import type { 
  ContractReceipt, 
  ContractEvent, 
  StateDiff, 
  TouchedKey,
  StateWitness,
  ContractReceiptWithWitness
} from './types.js';

export async function createCallId(txHash: string, checkpointHeight: number): Promise<string> {
  const data = `${txHash}:${checkpointHeight}`;
  return cryptoHash(data);
}

export async function computeEffectsHash(touchedKeys: TouchedKey[]): Promise<string> {
  const sorted = touchedKeys.sort((a, b) => {
    const keyA = `${a.contractId}:${a.key}`;
    const keyB = `${b.contractId}:${b.key}`;
    return keyA.localeCompare(keyB);
  });
  
  const data = sorted.map(tk => ({
    key: `${tk.contractId}:${tk.key}`,
    pre: tk.preValue,
    post: tk.postValue
  }));
  
  return cryptoHash(JSON.stringify(data));
}

export async function computeEventsHash(events: ContractEvent[]): Promise<string> {
  if (events.length === 0) {
    return cryptoHash('no-events');
  }
  
  const sorted = events.sort((a, b) => a.index - b.index);
  return cryptoHash(JSON.stringify(sorted));
}

export async function createContractReceipt(params: {
  txHash: string;
  checkpointHeight: number;
  contractId: string;
  entrypoint: string;
  caller: string;
  preStateRoot: string;
  postStateRoot: string;
  stateDiff: StateDiff | null;
  status: 'success' | 'revert' | 'out_of_gas';
  gasUsed: number;
  gasLimit: number;
  events: ContractEvent[];
  revertReason?: string;
}): Promise<ContractReceipt> {
  const callId = await createCallId(params.txHash, params.checkpointHeight);
  
  const touchedKeys: TouchedKey[] = params.stateDiff?.changes.map(change => ({
    contractId: params.contractId,
    key: change.key,
    preValue: change.oldValue,
    postValue: change.newValue
  })) || [];
  
  const effectsHash = await computeEffectsHash(touchedKeys);
  const eventsHash = await computeEventsHash(params.events);
  
  return {
    callId,
    txHash: params.txHash,
    contractId: params.contractId,
    entrypoint: params.entrypoint,
    caller: params.caller,
    preStateRoot: params.preStateRoot,
    postStateRoot: params.postStateRoot,
    effectsHash,
    status: params.status,
    gasUsed: params.gasUsed,
    gasLimit: params.gasLimit,
    eventsHash,
    eventCount: params.events.length,
    events: params.events.length > 0 ? params.events : undefined,
    revertReason: params.revertReason,
    executedAt: Date.now()
  };
}

export async function createReceiptWithWitness(
  receipt: ContractReceipt,
  witness: StateWitness
): Promise<ContractReceiptWithWitness> {
  return {
    ...receipt,
    witness
  };
}

export function parseLogToEvent(
  contractId: string,
  log: string,
  index: number
): ContractEvent {
  const match = log.match(/^(\w+):\s*(.+)$/);
  
  if (match) {
    return {
      contractId,
      eventName: match[1],
      data: tryParseJson(match[2]),
      index
    };
  }
  
  return {
    contractId,
    eventName: 'Log',
    data: { message: log },
    index
  };
}

function tryParseJson(str: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(str);
    if (typeof parsed === 'object' && parsed !== null) {
      return parsed;
    }
    return { value: parsed };
  } catch {
    return { message: str };
  }
}

export function compactReceipt(receipt: ContractReceipt): Omit<ContractReceipt, 'events'> {
  const { events, ...compact } = receipt;
  return compact;
}

export async function verifyReceiptEffects(
  receipt: ContractReceipt,
  touchedKeys: TouchedKey[]
): Promise<boolean> {
  const computedHash = await computeEffectsHash(touchedKeys);
  return computedHash === receipt.effectsHash;
}

export async function verifyReceiptEvents(
  receipt: ContractReceipt,
  events: ContractEvent[]
): Promise<boolean> {
  const computedHash = await computeEventsHash(events);
  return computedHash === receipt.eventsHash;
}
