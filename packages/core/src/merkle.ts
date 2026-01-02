import { hash } from './crypto.js';
import type { MerkleNode, AccountState } from './types.js';

export async function createMerkleTree(accounts: Map<string, AccountState>): Promise<MerkleNode | null> {
  const entries = Array.from(accounts.entries()).sort((a, b) => a[0].localeCompare(b[0]));
  
  if (entries.length === 0) {
    return null;
  }

  const leaves: MerkleNode[] = await Promise.all(
    entries.map(async ([fingerprint, state]) => ({
      hash: await hash(`${fingerprint}:${state.balance}:${state.nonce}`),
      data: fingerprint
    }))
  );

  return buildTreeFromLeaves(leaves);
}

async function buildTreeFromLeaves(leaves: MerkleNode[]): Promise<MerkleNode> {
  if (leaves.length === 1) {
    return leaves[0];
  }

  const nextLevel: MerkleNode[] = [];

  for (let i = 0; i < leaves.length; i += 2) {
    const left = leaves[i];
    const right = leaves[i + 1] || left;
    
    const combinedHash = await hash(left.hash + right.hash);
    
    nextLevel.push({
      hash: combinedHash,
      left,
      right: leaves[i + 1] ? right : undefined
    });
  }

  return buildTreeFromLeaves(nextLevel);
}

export async function getMerkleRoot(accounts: Map<string, AccountState>): Promise<string> {
  const tree = await createMerkleTree(accounts);
  return tree?.hash || await hash('empty');
}

export async function getMerkleProof(
  accounts: Map<string, AccountState>,
  fingerprint: string
): Promise<{ proof: string[]; index: number } | null> {
  const entries = Array.from(accounts.entries()).sort((a, b) => a[0].localeCompare(b[0]));
  const index = entries.findIndex(([fp]) => fp === fingerprint);
  
  if (index === -1) return null;

  const leaves: MerkleNode[] = await Promise.all(
    entries.map(async ([fp, state]) => ({
      hash: await hash(`${fp}:${state.balance}:${state.nonce}`),
      data: fp
    }))
  );

  const proof: string[] = [];
  let currentLevel = leaves;
  let currentIndex = index;

  while (currentLevel.length > 1) {
    const siblingIndex = currentIndex % 2 === 0 ? currentIndex + 1 : currentIndex - 1;
    
    if (siblingIndex < currentLevel.length) {
      proof.push(currentLevel[siblingIndex].hash);
    }

    const nextLevel: MerkleNode[] = [];
    for (let i = 0; i < currentLevel.length; i += 2) {
      const left = currentLevel[i];
      const right = currentLevel[i + 1] || left;
      const combinedHash = await hash(left.hash + right.hash);
      nextLevel.push({ hash: combinedHash });
    }

    currentLevel = nextLevel;
    currentIndex = Math.floor(currentIndex / 2);
  }

  return { proof, index };
}

export async function verifyMerkleProof(
  leafHash: string,
  proof: string[],
  index: number,
  root: string
): Promise<boolean> {
  let currentHash = leafHash;
  let currentIndex = index;

  for (const siblingHash of proof) {
    if (currentIndex % 2 === 0) {
      currentHash = await hash(currentHash + siblingHash);
    } else {
      currentHash = await hash(siblingHash + currentHash);
    }
    currentIndex = Math.floor(currentIndex / 2);
  }

  return currentHash === root;
}

export async function getTransactionMerkleRoot(txHashes: string[]): Promise<string> {
  if (txHashes.length === 0) {
    return await hash('empty-tx-tree');
  }

  const sorted = [...txHashes].sort();
  const leaves: MerkleNode[] = sorted.map(h => ({ hash: h }));
  const tree = await buildTreeFromLeaves(leaves);
  return tree.hash;
}

export async function getTransactionMerkleProof(
  txHashes: string[],
  targetHash: string
): Promise<{ proof: string[]; index: number } | null> {
  const sorted = [...txHashes].sort();
  const index = sorted.indexOf(targetHash);
  
  if (index === -1) return null;

  const leaves: MerkleNode[] = sorted.map(h => ({ hash: h }));
  const proof: string[] = [];
  let currentLevel = leaves;
  let currentIndex = index;

  while (currentLevel.length > 1) {
    const siblingIndex = currentIndex % 2 === 0 ? currentIndex + 1 : currentIndex - 1;
    
    if (siblingIndex < currentLevel.length) {
      proof.push(currentLevel[siblingIndex].hash);
    }

    const nextLevel: MerkleNode[] = [];
    for (let i = 0; i < currentLevel.length; i += 2) {
      const left = currentLevel[i];
      const right = currentLevel[i + 1] || left;
      const combinedHash = await hash(left.hash + right.hash);
      nextLevel.push({ hash: combinedHash });
    }

    currentLevel = nextLevel;
    currentIndex = Math.floor(currentIndex / 2);
  }

  return { proof, index };
}
