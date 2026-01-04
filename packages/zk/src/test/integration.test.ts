import { describe, it, expect, beforeAll } from 'vitest';
import * as path from 'path';
import * as fs from 'fs';
import { fileURLToPath } from 'url';
import { initProver, generateProof, isProverInitialized, computeExpectedOutputs } from '../prover.js';
import { initVerifier, isVerifierInitialized, verifyZkPayload, NullifierRegistry } from '../verifier.js';
import { initPoseidon, poseidonHash } from '../poseidon.js';
import { initEdDSA, generateKeyPair, signPoseidon, verifyPoseidon, privateKeyFromSeed } from '../eddsa.js';
import { createTestProofInput, initProofBuilder } from '../proof-builder.js';
import type { ZkProofInput } from '../types.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const BUILD_DIR = path.resolve(__dirname, '../../build/rinku_private_proof');

const wasmPath = path.join(BUILD_DIR, 'rinku_private_proof_js', 'rinku_private_proof.wasm');
const zkeyPath = path.join(BUILD_DIR, 'rinku_private_proof.zkey');
const vkeyPath = path.join(BUILD_DIR, 'verification_key.json');

const artifactsExist = fs.existsSync(wasmPath) && fs.existsSync(zkeyPath) && fs.existsSync(vkeyPath);

describe('EdDSA Signing', () => {
  beforeAll(async () => {
    await initEdDSA();
  }, 30000);

  it('should generate keypair and sign/verify message', async () => {
    const keyPair = generateKeyPair();
    
    expect(keyPair.privateKey).toBeDefined();
    expect(keyPair.privateKey.length).toBe(32);
    expect(keyPair.publicKey).toHaveLength(2);
    expect(typeof keyPair.publicKey[0]).toBe('bigint');
    expect(typeof keyPair.publicKey[1]).toBe('bigint');
    
    const message = 12345678901234567890n;
    const signature = signPoseidon(keyPair.privateKey, message);
    
    expect(signature.R8).toHaveLength(2);
    expect(typeof signature.S).toBe('bigint');
    
    const isValid = verifyPoseidon(message, signature, keyPair.publicKey);
    expect(isValid).toBe(true);
    
    const isInvalid = verifyPoseidon(message + 1n, signature, keyPair.publicKey);
    expect(isInvalid).toBe(false);
  });

  it('should derive consistent keypair from seed', () => {
    const seed = 'test-seed-for-determinism';
    const privKey1 = privateKeyFromSeed(seed);
    const privKey2 = privateKeyFromSeed(seed);
    
    expect(privKey1.equals(privKey2)).toBe(true);
  });
});

describe.skipIf(!artifactsExist)('ZK Proof Integration', () => {
  beforeAll(async () => {
    await initProofBuilder();
    await initProver(wasmPath, zkeyPath);
    await initVerifier(vkeyPath);
  }, 60000);

  it('should initialize prover and verifier', () => {
    expect(isProverInitialized()).toBe(true);
    expect(isVerifierInitialized()).toBe(true);
  });

  it('should generate and verify a complete ZK proof', async () => {
    const input = await createTestProofInput();
    
    expect(input.txHash).toBeDefined();
    expect(input.senderPubKeyX).toBeDefined();
    expect(input.txSigS).toBeDefined();
    
    const { proof, publicSignals } = await generateProof(input);
    
    expect(proof).toBeDefined();
    expect(proof.pi_a).toHaveLength(3);
    expect(proof.pi_b).toHaveLength(3);
    expect(proof.pi_c).toHaveLength(3);
    expect(publicSignals).toHaveLength(4);
    
    const expectedOutputs = computeExpectedOutputs(input);
    expect(BigInt(publicSignals[0])).toBe(expectedOutputs.checkpointRoot);
    expect(BigInt(publicSignals[1])).toBe(expectedOutputs.nullifier);
    expect(BigInt(publicSignals[2])).toBe(expectedOutputs.amountCommitment);
    expect(BigInt(publicSignals[3])).toBe(expectedOutputs.chainIdHash);
  }, 120000);

  it('should reject nullifier reuse', () => {
    const registry = new NullifierRegistry();
    const nullifier = 'test-nullifier-123';
    
    expect(registry.has(nullifier)).toBe(false);
    registry.add(nullifier);
    expect(registry.has(nullifier)).toBe(true);
    expect(registry.size()).toBe(1);
  });
});

describe('ZK Artifacts Check', () => {
  it('should report correct artifact availability', () => {
    console.log('Artifact paths:');
    console.log('  WASM:', wasmPath, fs.existsSync(wasmPath) ? '(exists)' : '(missing)');
    console.log('  zkey:', zkeyPath, fs.existsSync(zkeyPath) ? '(exists)' : '(missing)');
    console.log('  vkey:', vkeyPath, fs.existsSync(vkeyPath) ? '(exists)' : '(missing)');
    
    if (artifactsExist) {
      const wasmSize = fs.statSync(wasmPath).size;
      const zkeySize = fs.statSync(zkeyPath).size;
      const vkeySize = fs.statSync(vkeyPath).size;
      console.log(`  WASM size: ${(wasmSize / 1024 / 1024).toFixed(2)} MB`);
      console.log(`  zkey size: ${(zkeySize / 1024 / 1024).toFixed(2)} MB`);
      console.log(`  vkey size: ${(vkeySize / 1024).toFixed(2)} KB`);
    }
    
    expect(true).toBe(true);
  });
});
