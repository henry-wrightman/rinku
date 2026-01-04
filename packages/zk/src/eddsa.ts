import * as crypto from 'crypto';

let eddsa: any = null;
let babyJub: any = null;
let F: any = null;

export interface EdDSAKeyPair {
  privateKey: Buffer;
  publicKey: [bigint, bigint];
}

export interface EdDSASignature {
  R8: [bigint, bigint];
  S: bigint;
}

export async function initEdDSA(): Promise<void> {
  if (eddsa !== null) return;
  
  const circomlibjs = await import('circomlibjs');
  eddsa = await circomlibjs.buildEddsa();
  babyJub = await circomlibjs.buildBabyjub();
  F = babyJub.F;
}

export function isEdDSAInitialized(): boolean {
  return eddsa !== null;
}

export function generateKeyPair(): EdDSAKeyPair {
  if (!eddsa) throw new Error('EdDSA not initialized. Call initEdDSA() first.');
  
  const privateKey = crypto.randomBytes(32);
  const pubKey = eddsa.prv2pub(privateKey);
  
  return {
    privateKey,
    publicKey: [F.toObject(pubKey[0]), F.toObject(pubKey[1])]
  };
}

export function derivePublicKey(privateKey: Buffer): [bigint, bigint] {
  if (!eddsa) throw new Error('EdDSA not initialized. Call initEdDSA() first.');
  
  const pubKey = eddsa.prv2pub(privateKey);
  return [F.toObject(pubKey[0]), F.toObject(pubKey[1])];
}

export function signPoseidon(privateKey: Buffer, message: bigint): EdDSASignature {
  if (!eddsa) throw new Error('EdDSA not initialized. Call initEdDSA() first.');
  
  const msgF = F.e(message);
  const signature = eddsa.signPoseidon(privateKey, msgF);
  
  return {
    R8: [F.toObject(signature.R8[0]), F.toObject(signature.R8[1])],
    S: signature.S
  };
}

export function verifyPoseidon(message: bigint, signature: EdDSASignature, publicKey: [bigint, bigint]): boolean {
  if (!eddsa) throw new Error('EdDSA not initialized. Call initEdDSA() first.');
  
  const msgF = F.e(message);
  const sig = {
    R8: [F.e(signature.R8[0]), F.e(signature.R8[1])],
    S: signature.S
  };
  const pubKey = [F.e(publicKey[0]), F.e(publicKey[1])];
  
  return eddsa.verifyPoseidon(msgF, sig, pubKey);
}

export function privateKeyFromSeed(seed: string): Buffer {
  return crypto.createHash('sha256').update(seed).digest();
}
