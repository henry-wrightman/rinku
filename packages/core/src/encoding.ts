import pako from 'pako';
import type { Transaction, TransactionURL, ContractDeploy, ContractURL, ContractTransaction, SelfCrawlableBundle } from './types.js';

const base64urlChars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';

export function base64urlEncode(data: Uint8Array): string {
  let result = '';
  let bits = 0;
  let value = 0;

  for (let i = 0; i < data.length; i++) {
    value = (value << 8) | data[i];
    bits += 8;

    while (bits >= 6) {
      bits -= 6;
      result += base64urlChars[(value >> bits) & 0x3f];
    }
  }

  if (bits > 0) {
    result += base64urlChars[(value << (6 - bits)) & 0x3f];
  }

  return result;
}

export function base64urlDecode(str: string): Uint8Array {
  const bytes: number[] = [];
  let bits = 0;
  let value = 0;

  for (let i = 0; i < str.length; i++) {
    const idx = base64urlChars.indexOf(str[i]);
    if (idx === -1) continue;

    value = (value << 6) | idx;
    bits += 6;

    if (bits >= 8) {
      bits -= 8;
      bytes.push((value >> bits) & 0xff);
    }
  }

  return new Uint8Array(bytes);
}

export function encodeTransaction(tx: Transaction): string {
  const json = JSON.stringify(tx);
  const compressed = pako.deflate(json);
  return base64urlEncode(compressed);
}

export function decodeTransaction(payload: string): Transaction {
  const compressed = base64urlDecode(payload);
  const json = pako.inflate(compressed, { to: 'string' });
  return JSON.parse(json);
}

export function createTransactionURL(tx: Transaction): TransactionURL {
  const payload = encodeTransaction(tx);
  return {
    path: `/tx/${payload}`,
    payload
  };
}

export function parseTransactionURL(url: string): Transaction | null {
  try {
    const match = url.match(/\/tx\/([A-Za-z0-9_-]+)/);
    if (!match) return null;
    return decodeTransaction(match[1]);
  } catch {
    return null;
  }
}

// ============================================
// Smart Contract Encoding
// ============================================

/** Max URL length (most browsers support ~2000, we use conservative limit) */
const MAX_URL_LENGTH = 1500;

/** Encode contract deployment to URL payload */
export function encodeContractDeploy(deploy: ContractDeploy): string {
  const json = JSON.stringify(deploy);
  const compressed = pako.deflate(json);
  return base64urlEncode(compressed);
}

/** Decode contract deployment from URL payload */
export function decodeContractDeploy(payload: string): ContractDeploy {
  const compressed = base64urlDecode(payload);
  const json = pako.inflate(compressed, { to: 'string' });
  return JSON.parse(json);
}

/** Create contract deployment URL */
export function createContractURL(deploy: ContractDeploy): ContractURL {
  const payload = encodeContractDeploy(deploy);
  return {
    path: `/sc/${payload}`,
    payload
  };
}

/** Parse contract URL */
export function parseContractURL(url: string): ContractDeploy | null {
  try {
    const match = url.match(/\/sc\/([A-Za-z0-9_-]+)/);
    if (!match) return null;
    return decodeContractDeploy(match[1]);
  } catch {
    return null;
  }
}

/** Encode contract transaction (includes contract call data) */
export function encodeContractTransaction(tx: ContractTransaction): string {
  const json = JSON.stringify(tx);
  const compressed = pako.deflate(json);
  return base64urlEncode(compressed);
}

/** Decode contract transaction */
export function decodeContractTransaction(payload: string): ContractTransaction {
  const compressed = base64urlDecode(payload);
  const json = pako.inflate(compressed, { to: 'string' });
  return JSON.parse(json);
}

/** Check if a URL is within safe length limits */
export function isURLSafe(url: string): boolean {
  return url.length <= MAX_URL_LENGTH;
}

/** 
 * Chunk large WASM bytecode into multiple URLs.
 * Returns array of chunk URLs that can be referenced in deploy manifest.
 */
export function chunkWasmCode(wasmBase64: string, contractId: string): string[] {
  const chunkSize = 1000; // Base64 chars per chunk
  const chunks: string[] = [];
  
  for (let i = 0; i < wasmBase64.length; i += chunkSize) {
    const chunk = wasmBase64.slice(i, i + chunkSize);
    const index = Math.floor(i / chunkSize);
    chunks.push(`/sc/chunk/${contractId}/${index}/${chunk}`);
  }
  
  return chunks;
}

/** Reassemble WASM from chunk URLs */
export function assembleWasmFromChunks(chunkUrls: string[]): string {
  // Sort by index and extract chunk data
  const sorted = chunkUrls
    .map(url => {
      const match = url.match(/\/sc\/chunk\/[^/]+\/(\d+)\/(.+)/);
      if (!match) throw new Error(`Invalid chunk URL: ${url}`);
      return { index: parseInt(match[1]), data: match[2] };
    })
    .sort((a, b) => a.index - b.index);
  
  return sorted.map(c => c.data).join('');
}

/** Determine URL type from path */
export function getURLType(url: string): 'tx' | 'txp' | 'sc' | 'sc-chunk' | 'unknown' {
  if (url.startsWith('/tx/h/')) return 'tx';
  if (url.startsWith('/txp/')) return 'txp';
  if (url.startsWith('/tx/')) return 'tx';
  if (url.startsWith('/sc/chunk/')) return 'sc-chunk';
  if (url.startsWith('/sc/')) return 'sc';
  return 'unknown';
}

/** Encode self-crawlable bundle to URL payload */
export function encodeSelfCrawlableBundle(bundle: SelfCrawlableBundle): string {
  const json = JSON.stringify(bundle);
  const compressed = pako.deflate(json);
  return base64urlEncode(compressed);
}

/** Decode self-crawlable bundle from URL payload */
export function decodeSelfCrawlableBundle(payload: string): SelfCrawlableBundle {
  const compressed = base64urlDecode(payload);
  const json = pako.inflate(compressed, { to: 'string' });
  return JSON.parse(json);
}

/** Create self-crawlable proof URL */
export function createSelfCrawlableURL(bundle: SelfCrawlableBundle): { path: string; payload: string } {
  const payload = encodeSelfCrawlableBundle(bundle);
  return {
    path: `/txp/${payload}`,
    payload
  };
}

/** Parse self-crawlable proof URL */
export function parseSelfCrawlableURL(url: string): SelfCrawlableBundle | null {
  try {
    const match = url.match(/\/txp\/([A-Za-z0-9_-]+)/);
    if (!match) return null;
    return decodeSelfCrawlableBundle(match[1]);
  } catch {
    return null;
  }
}
