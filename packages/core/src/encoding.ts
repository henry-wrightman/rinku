import pako from 'pako';
import type { Transaction, TransactionURL } from './types.js';

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
