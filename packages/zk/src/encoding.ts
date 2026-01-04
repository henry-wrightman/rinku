import pako from 'pako';
import type { ZkProofPayload } from './types.js';

const ZK_URL_PREFIX = 'rinku://zk/';

export function encodeZkUrl(payload: ZkProofPayload): string {
  const json = JSON.stringify(payload);
  const compressed = pako.deflate(json);
  const base64 = base64UrlEncode(compressed);
  return `${ZK_URL_PREFIX}${base64}`;
}

export function decodeZkUrl(url: string): ZkProofPayload {
  if (!url.startsWith(ZK_URL_PREFIX)) {
    throw new Error(`Invalid ZK URL: must start with ${ZK_URL_PREFIX}`);
  }
  
  const base64 = url.slice(ZK_URL_PREFIX.length);
  const compressed = base64UrlDecode(base64);
  const json = pako.inflate(compressed, { to: 'string' });
  return JSON.parse(json) as ZkProofPayload;
}

export function isZkUrl(url: string): boolean {
  return url.startsWith(ZK_URL_PREFIX);
}

function base64UrlEncode(data: Uint8Array): string {
  let binary = '';
  for (let i = 0; i < data.length; i++) {
    binary += String.fromCharCode(data[i]);
  }
  return btoa(binary)
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/, '');
}

function base64UrlDecode(str: string): Uint8Array {
  let base64 = str.replace(/-/g, '+').replace(/_/g, '/');
  while (base64.length % 4) {
    base64 += '=';
  }
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

export function estimateUrlLength(payload: ZkProofPayload): number {
  const json = JSON.stringify(payload);
  const compressed = pako.deflate(json);
  const base64Length = Math.ceil(compressed.length * 4 / 3);
  return ZK_URL_PREFIX.length + base64Length;
}

export function fitsInQrCode(payload: ZkProofPayload, qrVersion: number = 15): boolean {
  const maxChars = getQrCapacity(qrVersion);
  return estimateUrlLength(payload) <= maxChars;
}

function getQrCapacity(version: number): number {
  const capacities: Record<number, number> = {
    10: 652,
    11: 772,
    12: 883,
    13: 1022,
    14: 1101,
    15: 1250,
    16: 1408,
    17: 1548,
    20: 2061,
    25: 3283,
    30: 4535,
    40: 7089
  };
  return capacities[version] || capacities[15];
}
