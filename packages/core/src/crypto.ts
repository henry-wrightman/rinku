import type { KeyPair, Transaction } from './types.js';

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export async function generateKeyPair(): Promise<KeyPair> {
  const keyPair = await crypto.subtle.generateKey(
    {
      name: 'ECDSA',
      namedCurve: 'P-256'
    },
    true,
    ['sign', 'verify']
  );

  const publicKeyRaw = await crypto.subtle.exportKey('raw', keyPair.publicKey);
  const privateKeyRaw = await crypto.subtle.exportKey('pkcs8', keyPair.privateKey);
  
  const fingerprint = await computeFingerprint(new Uint8Array(publicKeyRaw));

  return {
    publicKey: new Uint8Array(publicKeyRaw),
    privateKey: new Uint8Array(privateKeyRaw),
    fingerprint
  };
}

export async function computeFingerprint(publicKey: Uint8Array): Promise<string> {
  const hash = await crypto.subtle.digest('SHA-256', publicKey);
  return arrayToHex(new Uint8Array(hash)).slice(0, 40);
}

export async function sign(data: string, privateKey: Uint8Array): Promise<string> {
  const key = await crypto.subtle.importKey(
    'pkcs8',
    privateKey,
    { name: 'ECDSA', namedCurve: 'P-256' },
    false,
    ['sign']
  );

  const signature = await crypto.subtle.sign(
    { name: 'ECDSA', hash: 'SHA-256' },
    key,
    encoder.encode(data)
  );

  return arrayToHex(new Uint8Array(signature));
}

export async function verify(data: string, signature: string, publicKey: Uint8Array): Promise<boolean> {
  try {
    const key = await crypto.subtle.importKey(
      'raw',
      publicKey,
      { name: 'ECDSA', namedCurve: 'P-256' },
      false,
      ['verify']
    );

    return await crypto.subtle.verify(
      { name: 'ECDSA', hash: 'SHA-256' },
      key,
      hexToArray(signature),
      encoder.encode(data)
    );
  } catch {
    return false;
  }
}

export async function hash(data: string): Promise<string> {
  const hashBuffer = await crypto.subtle.digest('SHA-256', encoder.encode(data));
  return arrayToHex(new Uint8Array(hashBuffer));
}

export async function hashTransaction(tx: Transaction): Promise<string> {
  const txData = JSON.stringify({
    from: tx.from,
    to: tx.to,
    amount: tx.amount,
    nonce: tx.nonce,
    tipUrls: tx.tipUrls,
    ts: tx.ts
  });
  return hash(txData);
}

export function arrayToHex(arr: Uint8Array): string {
  return Array.from(arr)
    .map(b => b.toString(16).padStart(2, '0'))
    .join('');
}

export function hexToArray(hex: string): Uint8Array {
  const matches = hex.match(/.{1,2}/g);
  if (!matches) return new Uint8Array(0);
  return new Uint8Array(matches.map(byte => parseInt(byte, 16)));
}

export function serializeKeyPair(keyPair: KeyPair): string {
  return JSON.stringify({
    publicKey: arrayToHex(keyPair.publicKey),
    privateKey: arrayToHex(keyPair.privateKey),
    fingerprint: keyPair.fingerprint
  });
}

export function deserializeKeyPair(data: string): KeyPair {
  const parsed = JSON.parse(data);
  return {
    publicKey: hexToArray(parsed.publicKey),
    privateKey: hexToArray(parsed.privateKey),
    fingerprint: parsed.fingerprint
  };
}
