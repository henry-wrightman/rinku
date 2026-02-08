const P256_CURVE = { name: 'ECDSA', namedCurve: 'P-256' };
const HASH_ALGO = { name: 'SHA-256' };

function hexToBytes(hex: string): Uint8Array {
  const matches = hex.match(/.{1,2}/g);
  if (!matches) return new Uint8Array(0);
  return new Uint8Array(matches.map(byte => parseInt(byte, 16)));
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
}

async function sha256(data: Uint8Array): Promise<Uint8Array> {
  const hashBuffer = await crypto.subtle.digest('SHA-256', data.buffer as ArrayBuffer);
  return new Uint8Array(hashBuffer);
}

async function sha256Hex(data: string): Promise<string> {
  const encoder = new TextEncoder();
  const hashBuffer = await crypto.subtle.digest('SHA-256', encoder.encode(data));
  return bytesToHex(new Uint8Array(hashBuffer));
}

export interface SerializedKeyPair {
  publicKey: string;
  privateKey: string;
  fingerprint: string;
}

export async function generateKeyPair(): Promise<SerializedKeyPair> {
  const keyPair = await crypto.subtle.generateKey(P256_CURVE, true, ['sign', 'verify']);
  
  const publicKeyRaw = await crypto.subtle.exportKey('raw', keyPair.publicKey);
  const privateKeyPkcs8 = await crypto.subtle.exportKey('pkcs8', keyPair.privateKey);
  
  const publicKeyBytes = new Uint8Array(publicKeyRaw);
  const fingerprint = bytesToHex(await sha256(publicKeyBytes)).slice(0, 40);
  
  return {
    publicKey: bytesToHex(publicKeyBytes),
    privateKey: bytesToHex(new Uint8Array(privateKeyPkcs8)),
    fingerprint
  };
}

export function serializeKeyPair(kp: SerializedKeyPair): string {
  return JSON.stringify(kp);
}

export function deserializeKeyPair(data: string): SerializedKeyPair {
  const parsed = JSON.parse(data);
  return {
    publicKey: parsed.publicKey,
    privateKey: parsed.privateKey,
    fingerprint: parsed.fingerprint
  };
}

async function importPrivateKey(privateKeyHex: string): Promise<CryptoKey> {
  const privateKeyBytes = hexToBytes(privateKeyHex);
  return crypto.subtle.importKey(
    'pkcs8',
    privateKeyBytes.buffer as ArrayBuffer,
    P256_CURVE,
    false,
    ['sign']
  );
}

async function importPublicKey(publicKeyHex: string): Promise<CryptoKey> {
  const publicKeyBytes = hexToBytes(publicKeyHex);
  return crypto.subtle.importKey(
    'raw',
    publicKeyBytes.buffer as ArrayBuffer,
    P256_CURVE,
    false,
    ['verify']
  );
}

export async function signMessage(privateKeyHex: string, message: string): Promise<string> {
  const privateKey = await importPrivateKey(privateKeyHex);
  const encoder = new TextEncoder();
  const data = encoder.encode(message);
  
  const signature = await crypto.subtle.sign(
    { ...P256_CURVE, hash: HASH_ALGO },
    privateKey,
    data
  );
  
  return bytesToHex(new Uint8Array(signature));
}

export async function hashTransaction(txJson: string): Promise<string> {
  return sha256Hex(txJson);
}

export interface TransactionInner {
  from: string;
  to: string;
  amount: number;
  nonce: number;
  ts: number;
  parents: string[];
  kind?: 'transfer' | 'stake' | 'unstake' | 'contract' | 'consolidation' | 'reward';
  fee?: number;
  sig: string;
  hash: string;
  memo?: string;
  references?: string[];
}

export interface SignedTransaction {
  tx: TransactionInner;
}

export async function createSignedTransaction(
  keyPair: SerializedKeyPair,
  payload: { 
    to: string; 
    amount: number; 
    nonce: number; 
    parents: string[]; 
    kind?: string; 
    gasPrice?: number;
    memo?: string;
    references?: string[];
  }
): Promise<SignedTransaction> {
  const txData: Record<string, unknown> = {
    from: keyPair.fingerprint,
    to: payload.to,
    amount: payload.amount,
    nonce: payload.nonce,
    ts: Date.now(),
    parents: payload.parents,
    kind: payload.kind,
    fee: payload.gasPrice ?? 0.001,
  };
  
  if (payload.memo && payload.memo.trim()) {
    txData.memo = payload.memo.slice(0, 1024);
  }
  
  // Only include references if provided (max 4)
  if (payload.references && payload.references.length > 0) {
    txData.references = payload.references.slice(0, 4).filter(r => r.trim());
  }
  
  const txJson = JSON.stringify(txData);
  const hash = await hashTransaction(txJson);
  const sig = await signMessage(keyPair.privateKey, txJson);
  
  const tx: TransactionInner = {
    ...txData as Omit<TransactionInner, 'sig' | 'hash'>,
    sig,
    hash,
  };
  
  return { tx };
}

export function validateSerializedKey(data: string): boolean {
  try {
    const parsed = JSON.parse(data);
    return (
      typeof parsed.publicKey === 'string' &&
      typeof parsed.privateKey === 'string' &&
      typeof parsed.fingerprint === 'string' &&
      parsed.publicKey.length >= 64 &&
      parsed.privateKey.length >= 64 &&
      parsed.fingerprint.length === 40
    );
  } catch {
    return false;
  }
}

export function getFingerprint(data: string): string | null {
  try {
    const parsed = JSON.parse(data);
    return parsed.fingerprint || null;
  } catch {
    return null;
  }
}
