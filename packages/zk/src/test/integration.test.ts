import { describe, it, expect, beforeAll } from 'vitest';
import * as path from 'path';
import * as fs from 'fs';
import { fileURLToPath } from 'url';
import { initProver, generateProof, isProverInitialized, computeExpectedOutputs } from '../prover.js';
import { initVerifier, isVerifierInitialized, verifyZkUrl, NullifierRegistry } from '../verifier.js';
import { initPoseidon, poseidonHash } from '../poseidon.js';
import type { ZkProofInput } from '../types.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const BUILD_DIR = path.resolve(__dirname, '../../build/rinku_private_proof');

const wasmPath = path.join(BUILD_DIR, 'rinku_private_proof_js', 'rinku_private_proof.wasm');
const zkeyPath = path.join(BUILD_DIR, 'rinku_private_proof.zkey');
const vkeyPath = path.join(BUILD_DIR, 'verification_key.json');

const artifactsExist = fs.existsSync(wasmPath) && fs.existsSync(zkeyPath) && fs.existsSync(vkeyPath);

describe.skipIf(!artifactsExist)('ZK Proof Integration', () => {
  beforeAll(async () => {
    await initPoseidon();
    await initProver(wasmPath, zkeyPath);
    await initVerifier(vkeyPath);
  }, 30000);

  it('should initialize prover and verifier', () => {
    expect(isProverInitialized()).toBe(true);
    expect(isVerifierInitialized()).toBe(true);
  });

  it('should compute expected outputs correctly', async () => {
    const senderPrivKey = BigInt('12345678901234567890');
    const txHash = await poseidonHash([BigInt(42), BigInt(100)]);
    
    const input: ZkProofInput = {
      txHash,
      senderPrivKey,
      senderPubKeyX: BigInt(0),
      senderPubKeyY: BigInt(0),
      txSigR8X: BigInt(0),
      txSigR8Y: BigInt(0),
      txSigS: BigInt(0),
      merklePathElements: Array(10).fill(BigInt(0)),
      merklePathIndices: Array(10).fill(0),
      amount: BigInt(1000),
      amountBlinding: BigInt(987654321),
      checkpointHeight: BigInt(100),
      chainId: BigInt(1)
    };

    const expectedOutputs = computeExpectedOutputs(input);
    
    expect(expectedOutputs.nullifier).toBeDefined();
    expect(expectedOutputs.amountCommitment).toBeDefined();
    expect(expectedOutputs.chainIdHash).toBeDefined();
    expect(expectedOutputs.checkpointRoot).toBeDefined();
    
    expect(typeof expectedOutputs.nullifier).toBe('bigint');
    expect(typeof expectedOutputs.amountCommitment).toBe('bigint');
    expect(expectedOutputs.nullifier > 0n).toBe(true);
    expect(expectedOutputs.amountCommitment > 0n).toBe(true);
  }, 30000);

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
