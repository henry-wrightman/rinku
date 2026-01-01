import {
  generateKeyPair,
  serializeKeyPair,
  deserializeKeyPair,
  computeFingerprint,
  type KeyPair
} from '@rinku/core';

export class KeyManager {
  private keyPair: KeyPair | null = null;

  async create(): Promise<KeyPair> {
    this.keyPair = await generateKeyPair();
    return this.keyPair;
  }

  async import(serialized: string): Promise<KeyPair> {
    this.keyPair = deserializeKeyPair(serialized);
    return this.keyPair;
  }

  export(): string {
    if (!this.keyPair) {
      throw new Error('No key pair loaded');
    }
    return serializeKeyPair(this.keyPair);
  }

  getKeyPair(): KeyPair {
    if (!this.keyPair) {
      throw new Error('No key pair loaded');
    }
    return this.keyPair;
  }

  getFingerprint(): string {
    if (!this.keyPair) {
      throw new Error('No key pair loaded');
    }
    return this.keyPair.fingerprint;
  }

  getPublicKey(): Uint8Array {
    if (!this.keyPair) {
      throw new Error('No key pair loaded');
    }
    return this.keyPair.publicKey;
  }

  getPrivateKey(): Uint8Array {
    if (!this.keyPair) {
      throw new Error('No key pair loaded');
    }
    return this.keyPair.privateKey;
  }
}
