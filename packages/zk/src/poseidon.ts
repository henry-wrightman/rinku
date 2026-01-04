let poseidonInstance: ((inputs: bigint[]) => bigint) | null = null;

export async function initPoseidon(): Promise<void> {
  if (poseidonInstance) return;
  
  const { buildPoseidon } = await import('circomlibjs');
  const poseidon = await buildPoseidon();
  
  poseidonInstance = (inputs: bigint[]): bigint => {
    const hash = poseidon(inputs);
    return poseidon.F.toObject(hash);
  };
}

export function poseidonHash(inputs: bigint[]): bigint {
  if (!poseidonInstance) {
    throw new Error('Poseidon not initialized. Call initPoseidon() first.');
  }
  return poseidonInstance(inputs);
}

export function poseidonHash1(a: bigint): bigint {
  return poseidonHash([a]);
}

export function poseidonHash2(a: bigint, b: bigint): bigint {
  return poseidonHash([a, b]);
}

export function poseidonHash3(a: bigint, b: bigint, c: bigint): bigint {
  return poseidonHash([a, b, c]);
}

export function computeMerkleRoot(leaf: bigint, pathElements: bigint[], pathIndices: number[]): bigint {
  let current = leaf;
  
  for (let i = 0; i < pathElements.length; i++) {
    const sibling = pathElements[i];
    if (pathIndices[i] === 0) {
      current = poseidonHash2(current, sibling);
    } else {
      current = poseidonHash2(sibling, current);
    }
  }
  
  return current;
}

export function computeNullifier(senderPrivKey: bigint, checkpointHeight: bigint, txHash: bigint): bigint {
  return poseidonHash3(senderPrivKey, checkpointHeight, txHash);
}

export function computeAmountCommitment(amount: bigint, blinding: bigint): bigint {
  return poseidonHash2(amount, blinding);
}

export function computeChainIdHash(chainId: bigint): bigint {
  return poseidonHash1(chainId);
}
