import { describe, it, expect, beforeAll } from 'vitest';
import { initPoseidon, poseidonHash, poseidonHash2, computeMerkleRoot, computeNullifier } from '../poseidon.js';

describe('Poseidon Hash', () => {
  beforeAll(async () => {
    await initPoseidon();
  });

  it('should hash single input', () => {
    const result = poseidonHash([1n]);
    expect(typeof result).toBe('bigint');
    expect(result).toBeGreaterThan(0n);
  });

  it('should hash two inputs', () => {
    const result = poseidonHash2(1n, 2n);
    expect(typeof result).toBe('bigint');
    expect(result).toBeGreaterThan(0n);
  });

  it('should produce consistent hashes', () => {
    const hash1 = poseidonHash2(123n, 456n);
    const hash2 = poseidonHash2(123n, 456n);
    expect(hash1).toBe(hash2);
  });

  it('should produce different hashes for different inputs', () => {
    const hash1 = poseidonHash2(1n, 2n);
    const hash2 = poseidonHash2(2n, 1n);
    expect(hash1).not.toBe(hash2);
  });

  it('should compute Merkle root', () => {
    const leaf = 12345n;
    const pathElements = [1n, 2n, 3n];
    const pathIndices = [0, 1, 0];
    
    const root = computeMerkleRoot(leaf, pathElements, pathIndices);
    expect(typeof root).toBe('bigint');
    expect(root).toBeGreaterThan(0n);
  });

  it('should compute nullifier', () => {
    const senderPrivKey = 12345n;
    const checkpointHeight = 100n;
    const txHash = 67890n;
    
    const nullifier = computeNullifier(senderPrivKey, checkpointHeight, txHash);
    expect(typeof nullifier).toBe('bigint');
    expect(nullifier).toBeGreaterThan(0n);
    
    const nullifier2 = computeNullifier(senderPrivKey, checkpointHeight, txHash);
    expect(nullifier).toBe(nullifier2);
  });
});
