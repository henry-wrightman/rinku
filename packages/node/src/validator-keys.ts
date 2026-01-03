import { generateKeyPair, sign, verify, computeFingerprint } from '@rinku/core';
import type { ValidatorEntry, ValidatorSignature } from '@rinku/core';
import { createCipheriv, createDecipheriv, randomBytes, scryptSync, createHash } from 'crypto';

export interface ValidatorKeyPair {
  address: string;
  publicKey: Uint8Array;
  privateKey: Uint8Array;
  createdAt: number;
}

export interface ValidatorKeyManagerConfig {
  keyRotationIntervalMs?: number;
  maxKeyHistory?: number;
  encryptionPassword?: string;
}

const DEFAULT_CONFIG: Required<Omit<ValidatorKeyManagerConfig, 'encryptionPassword'>> = {
  keyRotationIntervalMs: 7 * 24 * 60 * 60 * 1000,
  maxKeyHistory: 5,
};

const ENCRYPTION_ALGORITHM = 'aes-256-gcm';
const SCRYPT_N = 16384;
const SCRYPT_R = 8;
const SCRYPT_P = 1;
const SALT_LENGTH = 32;
const IV_LENGTH = 16;
const AUTH_TAG_LENGTH = 16;

function deriveKey(password: string, salt: Buffer): Buffer {
  return scryptSync(password, salt, 32, { N: SCRYPT_N, r: SCRYPT_R, p: SCRYPT_P });
}

function encryptPrivateKey(privateKey: Uint8Array, password: string): string {
  const salt = randomBytes(SALT_LENGTH);
  const iv = randomBytes(IV_LENGTH);
  const key = deriveKey(password, salt);
  
  const cipher = createCipheriv(ENCRYPTION_ALGORITHM, key, iv);
  const encrypted = Buffer.concat([
    cipher.update(Buffer.from(privateKey)),
    cipher.final()
  ]);
  const authTag = cipher.getAuthTag();
  
  const combined = Buffer.concat([salt, iv, authTag, encrypted]);
  return combined.toString('base64');
}

function decryptPrivateKey(encryptedData: string, password: string): Uint8Array {
  const combined = Buffer.from(encryptedData, 'base64');
  
  const salt = combined.subarray(0, SALT_LENGTH);
  const iv = combined.subarray(SALT_LENGTH, SALT_LENGTH + IV_LENGTH);
  const authTag = combined.subarray(SALT_LENGTH + IV_LENGTH, SALT_LENGTH + IV_LENGTH + AUTH_TAG_LENGTH);
  const encrypted = combined.subarray(SALT_LENGTH + IV_LENGTH + AUTH_TAG_LENGTH);
  
  const key = deriveKey(password, salt);
  
  const decipher = createDecipheriv(ENCRYPTION_ALGORITHM, key, iv);
  decipher.setAuthTag(authTag);
  
  const decrypted = Buffer.concat([
    decipher.update(encrypted),
    decipher.final()
  ]);
  
  return new Uint8Array(decrypted);
}

function hashPassword(password: string): string {
  return createHash('sha256').update(password).digest('hex').slice(0, 16);
}

export interface ValidatorKeySnapshot {
  activeKey: {
    address: string;
    publicKey: number[];
    encryptedPrivateKey: string;
    createdAt: number;
  } | null;
  keyHistory: Array<{
    address: string;
    publicKey: number[];
    createdAt: number;
    retiredAt: number;
  }>;
  registeredValidators: Array<{
    address: string;
    publicKey: number[];
    weight: number;
    registeredAt: number;
  }>;
  passwordHash?: string;
}

export class ValidatorKeyManager {
  private activeKey: ValidatorKeyPair | null = null;
  private keyHistory: Array<{
    address: string;
    publicKey: Uint8Array;
    createdAt: number;
    retiredAt: number;
  }> = [];
  private registeredValidators: Map<string, {
    publicKey: Uint8Array;
    weight: number;
    registeredAt: number;
  }> = new Map();
  private config: ValidatorKeyManagerConfig & Required<Omit<ValidatorKeyManagerConfig, 'encryptionPassword'>>;
  private encryptionPassword: string;

  constructor(config: ValidatorKeyManagerConfig = {}) {
    const { encryptionPassword, ...rest } = config;
    this.config = { ...DEFAULT_CONFIG, ...rest };
    const envPassword = process.env.VALIDATOR_KEY_PASSWORD;
    
    if (encryptionPassword) {
      this.encryptionPassword = encryptionPassword;
    } else if (envPassword) {
      this.encryptionPassword = envPassword;
    } else {
      const isProduction = process.env.NODE_ENV === 'production';
      if (isProduction) {
        throw new Error('VALIDATOR_KEY_PASSWORD environment variable is required in production');
      }
      this.encryptionPassword = 'rinku-dev-key-password-do-not-use-in-production';
      console.warn('Using default development password for validator keys - set VALIDATOR_KEY_PASSWORD for production');
    }
  }

  async generateNewKey(): Promise<ValidatorKeyPair> {
    const { publicKey, privateKey } = await generateKeyPair();
    const address = await computeFingerprint(publicKey);

    const keyPair: ValidatorKeyPair = {
      address,
      publicKey,
      privateKey,
      createdAt: Date.now(),
    };

    if (this.activeKey) {
      this.keyHistory.push({
        address: this.activeKey.address,
        publicKey: this.activeKey.publicKey,
        createdAt: this.activeKey.createdAt,
        retiredAt: Date.now(),
      });

      if (this.keyHistory.length > this.config.maxKeyHistory) {
        this.keyHistory = this.keyHistory.slice(-this.config.maxKeyHistory);
      }
    }

    this.activeKey = keyPair;
    return keyPair;
  }

  async importKey(privateKey: Uint8Array, publicKey: Uint8Array): Promise<ValidatorKeyPair> {
    const address = await computeFingerprint(publicKey);

    const keyPair: ValidatorKeyPair = {
      address,
      publicKey,
      privateKey,
      createdAt: Date.now(),
    };

    if (this.activeKey) {
      this.keyHistory.push({
        address: this.activeKey.address,
        publicKey: this.activeKey.publicKey,
        createdAt: this.activeKey.createdAt,
        retiredAt: Date.now(),
      });
    }

    this.activeKey = keyPair;
    return keyPair;
  }

  getActiveKey(): ValidatorKeyPair | null {
    return this.activeKey;
  }

  getPublicKey(): Uint8Array | null {
    return this.activeKey?.publicKey || null;
  }

  getPrivateKey(): Uint8Array | null {
    return this.activeKey?.privateKey || null;
  }

  getAddress(): string | null {
    return this.activeKey?.address || null;
  }

  async signData(data: string): Promise<string> {
    if (!this.activeKey) {
      throw new Error('No active validator key');
    }
    return sign(data, this.activeKey.privateKey);
  }

  async signCheckpointData(
    checkpointData: {
      checkpointId: string;
      height: number;
      merkleRoot: string;
      stateRoot?: string;
      receiptRoot?: string;
    },
    weight: number
  ): Promise<ValidatorSignature> {
    if (!this.activeKey) {
      throw new Error('No active validator key');
    }

    const signingData = JSON.stringify({
      checkpointId: checkpointData.checkpointId,
      height: checkpointData.height,
      merkleRoot: checkpointData.merkleRoot,
      stateRoot: checkpointData.stateRoot,
      receiptRoot: checkpointData.receiptRoot,
    }) + `:${weight}`;

    const signature = await sign(signingData, this.activeKey.privateKey);

    return {
      validator: this.activeKey.address,
      signature,
      publicKey: Array.from(this.activeKey.publicKey),
      weight,
      timestamp: Date.now(),
    };
  }

  async verifySignature(data: string, signature: string, publicKey: Uint8Array): Promise<boolean> {
    return verify(data, signature, publicKey);
  }

  registerValidator(address: string, publicKey: Uint8Array, weight: number): void {
    this.registeredValidators.set(address, {
      publicKey,
      weight,
      registeredAt: Date.now(),
    });
  }

  unregisterValidator(address: string): boolean {
    return this.registeredValidators.delete(address);
  }

  getRegisteredValidator(address: string): { publicKey: Uint8Array; weight: number } | null {
    const entry = this.registeredValidators.get(address);
    if (!entry) return null;
    return { publicKey: entry.publicKey, weight: entry.weight };
  }

  getRegisteredValidators(): ValidatorEntry[] {
    return Array.from(this.registeredValidators.entries()).map(([address, info]) => ({
      address,
      publicKey: Array.from(info.publicKey),
      weight: info.weight,
    }));
  }

  updateValidatorWeight(address: string, weight: number): boolean {
    const existing = this.registeredValidators.get(address);
    if (!existing) return false;
    existing.weight = weight;
    return true;
  }

  async verifyValidatorSignature(
    address: string,
    data: string,
    signature: string
  ): Promise<{ valid: boolean; error?: string }> {
    const validator = this.registeredValidators.get(address);
    if (!validator) {
      return { valid: false, error: 'Unknown validator' };
    }

    try {
      const isValid = await verify(data, signature, validator.publicKey);
      return { valid: isValid, error: isValid ? undefined : 'Invalid signature' };
    } catch (e: any) {
      return { valid: false, error: e.message };
    }
  }

  needsKeyRotation(): boolean {
    if (!this.activeKey) return true;
    const age = Date.now() - this.activeKey.createdAt;
    return age >= this.config.keyRotationIntervalMs;
  }

  toJSON(): ValidatorKeySnapshot {
    return {
      activeKey: this.activeKey ? {
        address: this.activeKey.address,
        publicKey: Array.from(this.activeKey.publicKey),
        encryptedPrivateKey: encryptPrivateKey(this.activeKey.privateKey, this.encryptionPassword),
        createdAt: this.activeKey.createdAt,
      } : null,
      keyHistory: this.keyHistory.map(k => ({
        address: k.address,
        publicKey: Array.from(k.publicKey),
        createdAt: k.createdAt,
        retiredAt: k.retiredAt,
      })),
      registeredValidators: Array.from(this.registeredValidators.entries()).map(([address, info]) => ({
        address,
        publicKey: Array.from(info.publicKey),
        weight: info.weight,
        registeredAt: info.registeredAt,
      })),
      passwordHash: hashPassword(this.encryptionPassword),
    };
  }

  static async fromJSON(data: ValidatorKeySnapshot, password?: string): Promise<ValidatorKeyManager> {
    const manager = new ValidatorKeyManager({ 
      encryptionPassword: password || undefined 
    });
    
    for (const v of data.registeredValidators) {
      manager.registeredValidators.set(v.address, {
        publicKey: new Uint8Array(v.publicKey),
        weight: v.weight,
        registeredAt: v.registeredAt,
      });
    }
    
    manager.keyHistory = data.keyHistory.map(k => ({
      address: k.address,
      publicKey: new Uint8Array(k.publicKey),
      createdAt: k.createdAt,
      retiredAt: k.retiredAt,
    }));
    
    if (data.passwordHash && hashPassword(manager.encryptionPassword) !== data.passwordHash) {
      console.warn('VALIDATOR_KEY_PASSWORD changed - generating new validator key (old key lost)');
      await manager.generateNewKey();
      return manager;
    }

    if (data.activeKey && data.activeKey.encryptedPrivateKey) {
      try {
        const decryptedPrivateKey = decryptPrivateKey(data.activeKey.encryptedPrivateKey, manager.encryptionPassword);
        manager.activeKey = {
          address: data.activeKey.address,
          publicKey: new Uint8Array(data.activeKey.publicKey),
          privateKey: decryptedPrivateKey,
          createdAt: data.activeKey.createdAt,
        };
        console.log(`Restored validator key: ${manager.activeKey.address.slice(0, 16)}...`);
      } catch (e) {
        console.warn('Failed to decrypt validator key - generating new key');
        await manager.generateNewKey();
      }
    } else {
      console.log('No existing validator key in snapshot - generating new key');
      await manager.generateNewKey();
    }

    return manager;
  }
}
