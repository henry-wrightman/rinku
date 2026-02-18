const HKDF_HASH = 'SHA-256';
const AES_KEY_LENGTH = 256;
const IV_LENGTH = 12;

function hexToBytes(hex: string): Uint8Array {
  const matches = hex.match(/.{1,2}/g);
  if (!matches) return new Uint8Array(0);
  return new Uint8Array(matches.map(byte => parseInt(byte, 16)));
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('');
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = '';
  for (let i = 0; i < bytes.length; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary);
}

function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

export interface ECDHKeyPair {
  publicKey: string;
  privateKeyObj: CryptoKey;
}

export async function generateECDHKeyPair(): Promise<ECDHKeyPair> {
  const keyPair = await crypto.subtle.generateKey(
    { name: 'ECDH', namedCurve: 'P-256' },
    false,
    ['deriveBits']
  );
  const publicKeyRaw = await crypto.subtle.exportKey('raw', keyPair.publicKey);
  return {
    publicKey: bytesToHex(new Uint8Array(publicKeyRaw)),
    privateKeyObj: keyPair.privateKey,
  };
}

export async function exportECDHPrivateKey(key: CryptoKey): Promise<string> {
  const exported = await crypto.subtle.exportKey('pkcs8', key);
  return bytesToHex(new Uint8Array(exported));
}

export async function generateExportableECDHKeyPair(): Promise<{ publicKey: string; privateKey: string }> {
  const keyPair = await crypto.subtle.generateKey(
    { name: 'ECDH', namedCurve: 'P-256' },
    true,
    ['deriveBits']
  );
  const publicKeyRaw = await crypto.subtle.exportKey('raw', keyPair.publicKey);
  const privateKeyPkcs8 = await crypto.subtle.exportKey('pkcs8', keyPair.privateKey);
  return {
    publicKey: bytesToHex(new Uint8Array(publicKeyRaw)),
    privateKey: bytesToHex(new Uint8Array(privateKeyPkcs8)),
  };
}

async function importECDHPublicKey(publicKeyHex: string): Promise<CryptoKey> {
  const publicKeyBytes = hexToBytes(publicKeyHex);
  return crypto.subtle.importKey(
    'raw',
    publicKeyBytes.buffer as ArrayBuffer,
    { name: 'ECDH', namedCurve: 'P-256' },
    false,
    []
  );
}

async function importECDHPrivateKey(privateKeyHex: string): Promise<CryptoKey> {
  const privateKeyBytes = hexToBytes(privateKeyHex);
  return crypto.subtle.importKey(
    'pkcs8',
    privateKeyBytes.buffer as ArrayBuffer,
    { name: 'ECDH', namedCurve: 'P-256' },
    false,
    ['deriveBits']
  );
}

async function deriveSharedSecret(privateKey: CryptoKey, publicKeyHex: string): Promise<Uint8Array> {
  const publicKey = await importECDHPublicKey(publicKeyHex);
  const sharedBits = await crypto.subtle.deriveBits(
    { name: 'ECDH', public: publicKey },
    privateKey,
    256
  );
  return new Uint8Array(sharedBits);
}

async function hkdfDerive(
  inputKeyMaterial: Uint8Array,
  salt: Uint8Array,
  info: string,
  length: number = 32
): Promise<Uint8Array> {
  const baseKey = await crypto.subtle.importKey(
    'raw',
    inputKeyMaterial.buffer as ArrayBuffer,
    'HKDF',
    false,
    ['deriveBits']
  );
  const encoder = new TextEncoder();
  const derived = await crypto.subtle.deriveBits(
    {
      name: 'HKDF',
      hash: HKDF_HASH,
      salt: salt.buffer as ArrayBuffer,
      info: encoder.encode(info),
    },
    baseKey,
    length * 8
  );
  return new Uint8Array(derived);
}

async function aesGcmEncrypt(key: Uint8Array, plaintext: string): Promise<{ iv: string; ciphertext: string }> {
  const iv = crypto.getRandomValues(new Uint8Array(IV_LENGTH));
  const aesKey = await crypto.subtle.importKey(
    'raw',
    key.buffer as ArrayBuffer,
    { name: 'AES-GCM', length: AES_KEY_LENGTH },
    false,
    ['encrypt']
  );
  const encoder = new TextEncoder();
  const encrypted = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv },
    aesKey,
    encoder.encode(plaintext)
  );
  return {
    iv: bytesToBase64(iv),
    ciphertext: bytesToBase64(new Uint8Array(encrypted)),
  };
}

async function aesGcmDecrypt(key: Uint8Array, iv: string, ciphertext: string): Promise<string> {
  const ivBytes = base64ToBytes(iv);
  const ctBytes = base64ToBytes(ciphertext);
  const aesKey = await crypto.subtle.importKey(
    'raw',
    key.buffer as ArrayBuffer,
    { name: 'AES-GCM', length: AES_KEY_LENGTH },
    false,
    ['decrypt']
  );
  const decrypted = await crypto.subtle.decrypt(
    { name: 'AES-GCM', iv: ivBytes.buffer as ArrayBuffer },
    aesKey,
    ctBytes.buffer as ArrayBuffer
  );
  return new TextDecoder().decode(decrypted);
}

export interface RatchetState {
  rootKey: string;
  sendChainKey: string;
  recvChainKey: string;
  sendCount: number;
  recvCount: number;
  dhSendPublic: string;
  dhSendPrivate: string;
  dhRecvPublic: string;
  ratchetGeneration: number;
  established: boolean;
  totalSent: number;
  totalReceived: number;
}

export interface ChatSession {
  peerAddress: string;
  peerECDHPublic: string;
  myECDHPublic: string;
  myECDHPrivate: string;
  ratchet: RatchetState | null;
  status: 'pending_sent' | 'pending_received' | 'active';
  createdAt: number;
  lastMessageAt: number;
}

export interface EncryptedEnvelope {
  type: 'dm';
  v: 1;
  dh: string;
  n: number;
  iv: string;
  ct: string;
}

export interface HandshakeEnvelope {
  type: 'dm_init' | 'dm_accept';
  v: 1;
  ecdhPub: string;
  from: string;
}

const SESSIONS_PREFIX = 'rinku_chat_sessions_';
const MSG_CACHE_PREFIX = 'rinku_chat_msgs_';

let _currentWallet: string | null = null;

export function setCurrentWallet(address: string | null): void {
  _currentWallet = address;
}

function sessionsKey(): string {
  if (!_currentWallet) throw new Error('No wallet set for chat storage');
  return SESSIONS_PREFIX + _currentWallet;
}

function msgCacheKey(peerAddress: string): string {
  if (!_currentWallet) throw new Error('No wallet set for chat storage');
  return MSG_CACHE_PREFIX + _currentWallet + '_' + peerAddress;
}

export function loadSessions(): Record<string, ChatSession> {
  if (!_currentWallet) return {};
  try {
    const stored = localStorage.getItem(sessionsKey());
    if (stored) {
      const sessions: Record<string, ChatSession> = JSON.parse(stored);
      let migrated = false;
      for (const key of Object.keys(sessions)) {
        const r = sessions[key].ratchet;
        if (r && r.totalSent === undefined) {
          r.totalSent = r.sendCount || 0;
          r.totalReceived = r.recvCount || 0;
          migrated = true;
        }
      }
      if (migrated) {
        localStorage.setItem(sessionsKey(), JSON.stringify(sessions));
      }
      return sessions;
    }
  } catch {}
  return {};
}

export function saveSessions(sessions: Record<string, ChatSession>): void {
  localStorage.setItem(sessionsKey(), JSON.stringify(sessions));
}

export function getSession(peerAddress: string): ChatSession | null {
  if (!_currentWallet) return null;
  const sessions = loadSessions();
  return sessions[peerAddress] || null;
}

export function saveSession(peerAddress: string, session: ChatSession): void {
  if (!_currentWallet) return;
  const sessions = loadSessions();
  sessions[peerAddress] = session;
  saveSessions(sessions);
}

export function deleteSession(peerAddress: string): void {
  if (!_currentWallet) return;
  const sessions = loadSessions();
  delete sessions[peerAddress];
  saveSessions(sessions);
  localStorage.removeItem(msgCacheKey(peerAddress));
  dismissPeer(peerAddress);
}

const DISMISSED_PREFIX = 'rinku_chat_dismissed_';

function dismissedKey(): string {
  return DISMISSED_PREFIX + _currentWallet;
}

export function dismissPeer(peerAddress: string): void {
  if (!_currentWallet) return;
  try {
    const stored = localStorage.getItem(dismissedKey());
    const dismissed: Record<string, number> = stored ? JSON.parse(stored) : {};
    dismissed[peerAddress] = Date.now();
    localStorage.setItem(dismissedKey(), JSON.stringify(dismissed));
  } catch {}
}

export function getDismissedAt(peerAddress: string): number | null {
  if (!_currentWallet) return null;
  try {
    const stored = localStorage.getItem(dismissedKey());
    if (stored) {
      const dismissed: Record<string, number> = JSON.parse(stored);
      return dismissed[peerAddress] || null;
    }
  } catch {}
  return null;
}

export function clearDismissed(peerAddress: string): void {
  if (!_currentWallet) return;
  try {
    const stored = localStorage.getItem(dismissedKey());
    if (stored) {
      const dismissed: Record<string, number> = JSON.parse(stored);
      delete dismissed[peerAddress];
      localStorage.setItem(dismissedKey(), JSON.stringify(dismissed));
    }
  } catch {}
}

export function getCachedPlaintext(peerAddress: string, txHash: string): string | undefined {
  if (!_currentWallet) return undefined;
  try {
    const stored = localStorage.getItem(msgCacheKey(peerAddress));
    if (stored) {
      const cache: Record<string, string> = JSON.parse(stored);
      return cache[txHash];
    }
  } catch {}
  return undefined;
}

export function setCachedPlaintext(peerAddress: string, txHash: string, plaintext: string): void {
  if (!_currentWallet) return;
  try {
    const stored = localStorage.getItem(msgCacheKey(peerAddress));
    const cache: Record<string, string> = stored ? JSON.parse(stored) : {};
    cache[txHash] = plaintext;
    localStorage.setItem(msgCacheKey(peerAddress), JSON.stringify(cache));
  } catch {}
}

export async function initiateHandshake(
  myAddress: string,
  peerAddress: string
): Promise<{ session: ChatSession; envelope: HandshakeEnvelope }> {
  const kp = await generateExportableECDHKeyPair();
  const session: ChatSession = {
    peerAddress,
    peerECDHPublic: '',
    myECDHPublic: kp.publicKey,
    myECDHPrivate: kp.privateKey,
    ratchet: null,
    status: 'pending_sent',
    createdAt: Date.now(),
    lastMessageAt: Date.now(),
  };
  saveSession(peerAddress, session);

  const envelope: HandshakeEnvelope = {
    type: 'dm_init',
    v: 1,
    ecdhPub: kp.publicKey,
    from: myAddress,
  };
  return { session, envelope };
}

export async function acceptHandshake(
  myAddress: string,
  peerAddress: string,
  peerECDHPublic: string
): Promise<{ session: ChatSession; envelope: HandshakeEnvelope }> {
  const kp = await generateExportableECDHKeyPair();

  const myPrivKey = await importECDHPrivateKey(kp.privateKey);
  const sharedSecret = await deriveSharedSecret(myPrivKey, peerECDHPublic);

  const salt = new TextEncoder().encode('rinku-dm-v1');
  const rootKey = await hkdfDerive(sharedSecret, salt, 'root-key', 32);
  const sendChainKey = await hkdfDerive(rootKey, salt, 'send-chain', 32);
  const recvChainKey = await hkdfDerive(rootKey, salt, 'recv-chain', 32);

  const ratchet: RatchetState = {
    rootKey: bytesToHex(rootKey),
    sendChainKey: bytesToHex(sendChainKey),
    recvChainKey: bytesToHex(recvChainKey),
    sendCount: 0,
    recvCount: 0,
    dhSendPublic: kp.publicKey,
    dhSendPrivate: kp.privateKey,
    dhRecvPublic: peerECDHPublic,
    ratchetGeneration: 0,
    established: true,
    totalSent: 0,
    totalReceived: 0,
  };

  const session: ChatSession = {
    peerAddress,
    peerECDHPublic,
    myECDHPublic: kp.publicKey,
    myECDHPrivate: kp.privateKey,
    ratchet,
    status: 'active',
    createdAt: Date.now(),
    lastMessageAt: Date.now(),
  };
  saveSession(peerAddress, session);

  const envelope: HandshakeEnvelope = {
    type: 'dm_accept',
    v: 1,
    ecdhPub: kp.publicKey,
    from: myAddress,
  };
  return { session, envelope };
}

export async function completeHandshake(
  peerAddress: string,
  peerECDHPublic: string
): Promise<ChatSession> {
  const session = getSession(peerAddress);
  if (!session) throw new Error('No pending session for ' + peerAddress);

  const myPrivKey = await importECDHPrivateKey(session.myECDHPrivate);
  const sharedSecret = await deriveSharedSecret(myPrivKey, peerECDHPublic);

  const salt = new TextEncoder().encode('rinku-dm-v1');
  const rootKey = await hkdfDerive(sharedSecret, salt, 'root-key', 32);
  const recvChainKey = await hkdfDerive(rootKey, salt, 'send-chain', 32);
  const sendChainKey = await hkdfDerive(rootKey, salt, 'recv-chain', 32);

  const ratchet: RatchetState = {
    rootKey: bytesToHex(rootKey),
    sendChainKey: bytesToHex(sendChainKey),
    recvChainKey: bytesToHex(recvChainKey),
    sendCount: 0,
    recvCount: 0,
    dhSendPublic: session.myECDHPublic,
    dhSendPrivate: session.myECDHPrivate,
    dhRecvPublic: peerECDHPublic,
    ratchetGeneration: 0,
    established: true,
    totalSent: 0,
    totalReceived: 0,
  };

  session.peerECDHPublic = peerECDHPublic;
  session.ratchet = ratchet;
  session.status = 'active';
  session.lastMessageAt = Date.now();
  saveSession(peerAddress, session);
  return session;
}

async function deriveMessageKey(chainKey: string, counter: number): Promise<{ messageKey: Uint8Array; nextChainKey: string }> {
  const chainKeyBytes = hexToBytes(chainKey);
  const salt = new TextEncoder().encode(`msg-${counter}`);
  const messageKey = await hkdfDerive(chainKeyBytes, salt, 'message-key', 32);
  const nextChainKeyBytes = await hkdfDerive(chainKeyBytes, salt, 'chain-advance', 32);
  return {
    messageKey,
    nextChainKey: bytesToHex(nextChainKeyBytes),
  };
}

export async function encryptMessage(
  peerAddress: string,
  plaintext: string
): Promise<EncryptedEnvelope> {
  const session = getSession(peerAddress);
  if (!session || !session.ratchet || session.status !== 'active') {
    throw new Error('No active session for ' + peerAddress);
  }

  const ratchet = session.ratchet;
  const { messageKey, nextChainKey } = await deriveMessageKey(ratchet.sendChainKey, ratchet.sendCount);
  const { iv, ciphertext } = await aesGcmEncrypt(messageKey, plaintext);

  const envelope: EncryptedEnvelope = {
    type: 'dm',
    v: 1,
    dh: ratchet.dhSendPublic,
    n: ratchet.sendCount,
    iv,
    ct: ciphertext,
  };

  ratchet.sendChainKey = nextChainKey;
  ratchet.sendCount++;
  ratchet.totalSent = (ratchet.totalSent || 0) + 1;
  session.lastMessageAt = Date.now();
  saveSession(peerAddress, session);

  messageKey.fill(0);

  return envelope;
}

export async function decryptMessage(
  peerAddress: string,
  envelope: EncryptedEnvelope,
  txHash?: string
): Promise<string> {
  const session = getSession(peerAddress);
  if (!session || !session.ratchet || session.status !== 'active') {
    throw new Error('No active session for ' + peerAddress);
  }

  const ratchet = session.ratchet;

  if (envelope.dh !== ratchet.dhRecvPublic) {
    const myPrivKey = await importECDHPrivateKey(ratchet.dhSendPrivate);
    const newSharedSecret = await deriveSharedSecret(myPrivKey, envelope.dh);
    const salt = new TextEncoder().encode('rinku-dm-v1');
    const newRootKey = await hkdfDerive(newSharedSecret, hexToBytes(ratchet.rootKey), 'dh-ratchet', 32);
    const newRecvChainKey = await hkdfDerive(newRootKey, salt, 'recv-ratchet', 32);

    ratchet.rootKey = bytesToHex(newRootKey);
    ratchet.recvChainKey = bytesToHex(newRecvChainKey);
    ratchet.dhRecvPublic = envelope.dh;
    ratchet.recvCount = 0;
    ratchet.ratchetGeneration++;
  }

  if (envelope.n < ratchet.recvCount) {
    throw new Error('Message already processed (counter ' + envelope.n + ' < recvCount ' + ratchet.recvCount + ')');
  }

  let chainKey = ratchet.recvChainKey;
  for (let i = ratchet.recvCount; i < envelope.n; i++) {
    const { nextChainKey } = await deriveMessageKey(chainKey, i);
    chainKey = nextChainKey;
  }

  const { messageKey, nextChainKey } = await deriveMessageKey(chainKey, envelope.n);

  let plaintext: string;
  try {
    plaintext = await aesGcmDecrypt(messageKey, envelope.iv, envelope.ct);
  } catch (e) {
    throw new Error('Failed to decrypt message - possible key mismatch or tampered data');
  }

  ratchet.recvChainKey = nextChainKey;
  ratchet.recvCount = envelope.n + 1;
  ratchet.totalReceived = (ratchet.totalReceived || 0) + 1;
  session.lastMessageAt = Date.now();
  saveSession(peerAddress, session);

  if (txHash) {
    setCachedPlaintext(peerAddress, txHash, plaintext);
  }

  messageKey.fill(0);

  return plaintext;
}

function hexToBase64(hex: string): string {
  return bytesToBase64(hexToBytes(hex));
}

function base64ToHex(b64: string): string {
  return bytesToHex(base64ToBytes(b64));
}

export function serializeEnvelope(env: EncryptedEnvelope | HandshakeEnvelope): string {
  if (env.type === 'dm') {
    const e = env as EncryptedEnvelope;
    return `DM|${hexToBase64(e.dh)}|${e.n}|${e.iv}|${e.ct}`;
  }
  if (env.type === 'dm_init') {
    const h = env as HandshakeEnvelope;
    return `DMI|${hexToBase64(h.ecdhPub)}|${h.from}`;
  }
  if (env.type === 'dm_accept') {
    const h = env as HandshakeEnvelope;
    return `DMA|${hexToBase64(h.ecdhPub)}|${h.from}`;
  }
  return JSON.stringify(env);
}

export function parseEnvelope(memo: string): EncryptedEnvelope | HandshakeEnvelope | null {
  if (memo.startsWith('DM|')) {
    const parts = memo.split('|');
    if (parts.length === 5) {
      return {
        type: 'dm',
        v: 1,
        dh: base64ToHex(parts[1]),
        n: parseInt(parts[2], 10),
        iv: parts[3],
        ct: parts[4],
      } as EncryptedEnvelope;
    }
  }
  if (memo.startsWith('DMI|')) {
    const parts = memo.split('|');
    if (parts.length === 3) {
      return {
        type: 'dm_init',
        v: 1,
        ecdhPub: base64ToHex(parts[1]),
        from: parts[2],
      } as HandshakeEnvelope;
    }
  }
  if (memo.startsWith('DMA|')) {
    const parts = memo.split('|');
    if (parts.length === 3) {
      return {
        type: 'dm_accept',
        v: 1,
        ecdhPub: base64ToHex(parts[1]),
        from: parts[2],
      } as HandshakeEnvelope;
    }
  }
  try {
    const parsed = JSON.parse(memo);
    if (parsed.type === 'dm' && parsed.v === 1) return parsed as EncryptedEnvelope;
    if ((parsed.type === 'dm_init' || parsed.type === 'dm_accept') && parsed.v === 1) return parsed as HandshakeEnvelope;
  } catch {}
  return null;
}

export function isHandshakeEnvelope(env: EncryptedEnvelope | HandshakeEnvelope): env is HandshakeEnvelope {
  return env.type === 'dm_init' || env.type === 'dm_accept';
}

export function isDMEnvelope(env: EncryptedEnvelope | HandshakeEnvelope): env is EncryptedEnvelope {
  return env.type === 'dm';
}

export function getForwardSecrecyInfo(peerAddress: string): {
  ratchetGeneration: number;
  messagesSent: number;
  messagesReceived: number;
  keyAge: number;
  established: boolean;
} | null {
  const session = getSession(peerAddress);
  if (!session || !session.ratchet) return null;
  return {
    ratchetGeneration: session.ratchet.ratchetGeneration,
    messagesSent: session.ratchet.totalSent || 0,
    messagesReceived: session.ratchet.totalReceived || 0,
    keyAge: Date.now() - session.lastMessageAt,
    established: session.ratchet.established,
  };
}
