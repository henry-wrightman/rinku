import { sha256 } from "@noble/hashes/sha2.js";
import { base64urlEncode, base64urlDecode } from "./encoding.js";

export interface MerkleSumNode {
  hash: string;
  sumWeight: number;
}

export interface MerkleSumLeaf {
  index: number;
  address: string;
  blsPublicKey: string;
  weight: number;
}

export interface MerkleSumProof {
  leaf: MerkleSumLeaf;
  siblings: MerkleSumNode[];
  pathBits: boolean[];
}

export interface MerkleSumRoot {
  hash: string;
  totalWeight: number;
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

function hashLeaf(leaf: MerkleSumLeaf): string {
  const data = `leaf:${leaf.index}:${leaf.address}:${leaf.blsPublicKey}:${leaf.weight}`;
  return bytesToHex(sha256(new TextEncoder().encode(data)));
}

function hashInternal(left: MerkleSumNode, right: MerkleSumNode): string {
  const data = `node:${left.hash}:${left.sumWeight}:${right.hash}:${right.sumWeight}`;
  return bytesToHex(sha256(new TextEncoder().encode(data)));
}

/**
 * Canonical empty node for non-power-of-two tree padding.
 * Uses domain-separated hash to ensure deterministic behavior across implementations.
 * Per spec Appendix A.3: EMPTY_NODE = { hash: SHA256("rinku:empty_node:v1"), sumWeight: 0 }
 */
export const EMPTY_NODE: MerkleSumNode = {
  hash: bytesToHex(sha256(new TextEncoder().encode("rinku:empty_node:v1"))),
  sumWeight: 0,
};

export function buildMerkleSumTree(leaves: MerkleSumLeaf[]): {
  root: MerkleSumRoot;
  layers: MerkleSumNode[][];
} {
  if (leaves.length === 0) {
    return {
      root: { hash: bytesToHex(sha256(new TextEncoder().encode("empty"))), totalWeight: 0 },
      layers: [],
    };
  }

  const sortedLeaves = [...leaves].sort((a, b) => a.index - b.index);

  let currentLayer: MerkleSumNode[] = sortedLeaves.map((leaf) => ({
    hash: hashLeaf(leaf),
    sumWeight: leaf.weight,
  }));

  const layers: MerkleSumNode[][] = [currentLayer];

  while (currentLayer.length > 1) {
    const nextLayer: MerkleSumNode[] = [];

    for (let i = 0; i < currentLayer.length; i += 2) {
      const left = currentLayer[i];
      const right = currentLayer[i + 1] || EMPTY_NODE;

      nextLayer.push({
        hash: hashInternal(left, right),
        sumWeight: left.sumWeight + right.sumWeight,
      });
    }

    currentLayer = nextLayer;
    layers.push(currentLayer);
  }

  const root = currentLayer[0];
  return {
    root: { hash: root.hash, totalWeight: root.sumWeight },
    layers,
  };
}

export function getMerkleSumProof(
  leaves: MerkleSumLeaf[],
  leafIndex: number
): MerkleSumProof | null {
  const sortedLeaves = [...leaves].sort((a, b) => a.index - b.index);
  const targetLeaf = sortedLeaves.find((l) => l.index === leafIndex);
  if (!targetLeaf) return null;

  const positionInArray = sortedLeaves.findIndex((l) => l.index === leafIndex);
  if (positionInArray === -1) return null;

  let currentLayer: MerkleSumNode[] = sortedLeaves.map((leaf) => ({
    hash: hashLeaf(leaf),
    sumWeight: leaf.weight,
  }));

  const siblings: MerkleSumNode[] = [];
  const pathBits: boolean[] = [];
  let pos = positionInArray;

  while (currentLayer.length > 1) {
    const isRight = pos % 2 === 1;
    const siblingPos = isRight ? pos - 1 : pos + 1;

    const sibling =
      siblingPos < currentLayer.length
        ? currentLayer[siblingPos]
        : EMPTY_NODE;

    siblings.push(sibling);
    pathBits.push(isRight);

    const nextLayer: MerkleSumNode[] = [];
    for (let i = 0; i < currentLayer.length; i += 2) {
      const left = currentLayer[i];
      const right = currentLayer[i + 1] || EMPTY_NODE;
      nextLayer.push({
        hash: hashInternal(left, right),
        sumWeight: left.sumWeight + right.sumWeight,
      });
    }

    pos = Math.floor(pos / 2);
    currentLayer = nextLayer;
  }

  return {
    leaf: targetLeaf,
    siblings,
    pathBits,
  };
}

export function verifyMerkleSumProof(
  proof: MerkleSumProof,
  expectedRoot: MerkleSumRoot
): { valid: boolean; leafWeight: number; errors: string[] } {
  const errors: string[] = [];

  let current: MerkleSumNode = {
    hash: hashLeaf(proof.leaf),
    sumWeight: proof.leaf.weight,
  };

  for (let i = 0; i < proof.siblings.length; i++) {
    const sibling = proof.siblings[i];
    const isRight = proof.pathBits[i];

    const left = isRight ? sibling : current;
    const right = isRight ? current : sibling;

    current = {
      hash: hashInternal(left, right),
      sumWeight: left.sumWeight + right.sumWeight,
    };
  }

  if (current.hash !== expectedRoot.hash) {
    errors.push(`Root hash mismatch: computed ${current.hash.slice(0, 16)}..., expected ${expectedRoot.hash.slice(0, 16)}...`);
  }

  if (current.sumWeight !== expectedRoot.totalWeight) {
    errors.push(`Total weight mismatch: computed ${current.sumWeight}, expected ${expectedRoot.totalWeight}`);
  }

  return {
    valid: errors.length === 0,
    leafWeight: proof.leaf.weight,
    errors,
  };
}

export function computeMerkleSumRootFromProofs(
  proofs: MerkleSumProof[]
): MerkleSumRoot | null {
  if (proofs.length === 0) return null;

  const roots: MerkleSumRoot[] = [];

  for (const proof of proofs) {
    let current: MerkleSumNode = {
      hash: hashLeaf(proof.leaf),
      sumWeight: proof.leaf.weight,
    };

    for (let i = 0; i < proof.siblings.length; i++) {
      const sibling = proof.siblings[i];
      const isRight = proof.pathBits[i];

      const left = isRight ? sibling : current;
      const right = isRight ? current : sibling;

      current = {
        hash: hashInternal(left, right),
        sumWeight: left.sumWeight + right.sumWeight,
      };
    }

    roots.push({ hash: current.hash, totalWeight: current.sumWeight });
  }

  const firstRoot = roots[0];
  for (let i = 1; i < roots.length; i++) {
    if (roots[i].hash !== firstRoot.hash || roots[i].totalWeight !== firstRoot.totalWeight) {
      return null;
    }
  }

  return firstRoot;
}

export function encodeMerkleSumProof(proof: MerkleSumProof): string {
  return base64urlEncode(
    new TextEncoder().encode(JSON.stringify(proof))
  );
}

export function decodeMerkleSumProof(encoded: string): MerkleSumProof {
  return JSON.parse(
    new TextDecoder().decode(base64urlDecode(encoded))
  );
}

export function encodeMerkleSumRoot(root: MerkleSumRoot): string {
  return `${root.hash}:${root.totalWeight}`;
}

export function decodeMerkleSumRoot(encoded: string): MerkleSumRoot {
  const [hash, weight] = encoded.split(":");
  return { hash, totalWeight: parseInt(weight, 10) };
}

export interface AuxiliaryNode {
  level: number;
  index: number;
  node: MerkleSumNode;
}

export interface MerkleSumMultiProof {
  committeeSize: number;  // N - total number of validators in committee
  leaves: MerkleSumLeaf[];
  auxiliaryNodes: AuxiliaryNode[];
}

export function getMerkleSumMultiProof(
  allLeaves: MerkleSumLeaf[],
  signerIndices: number[]
): MerkleSumMultiProof | null {
  if (signerIndices.length === 0) return null;

  const sortedLeaves = [...allLeaves].sort((a, b) => a.index - b.index);
  const signerSet = new Set(signerIndices);

  const signerLeaves = sortedLeaves.filter((l) => signerSet.has(l.index));
  if (signerLeaves.length !== signerIndices.length) return null;

  // Sort signer leaves by index for canonical ordering
  signerLeaves.sort((a, b) => a.index - b.index);

  let currentLayer: MerkleSumNode[] = sortedLeaves.map((leaf) => ({
    hash: hashLeaf(leaf),
    sumWeight: leaf.weight,
  }));

  const auxiliaryNodes: AuxiliaryNode[] = [];
  let coveredIndices = new Set(
    sortedLeaves
      .map((l, i) => (signerSet.has(l.index) ? i : -1))
      .filter((i) => i >= 0)
  );

  let level = 0;
  while (currentLayer.length > 1) {
    const nextCovered = new Set<number>();

    for (let i = 0; i < currentLayer.length; i += 2) {
      const leftCovered = coveredIndices.has(i);
      const rightCovered = coveredIndices.has(i + 1);
      const parentIdx = Math.floor(i / 2);

      if (leftCovered || rightCovered) {
        nextCovered.add(parentIdx);
      }

      if (leftCovered && !rightCovered && i + 1 < currentLayer.length) {
        auxiliaryNodes.push({
          level,
          index: i + 1,
          node: currentLayer[i + 1],
        });
      }

      if (rightCovered && !leftCovered) {
        auxiliaryNodes.push({
          level,
          index: i,
          node: currentLayer[i],
        });
      }
    }

    const nextLayer: MerkleSumNode[] = [];
    for (let i = 0; i < currentLayer.length; i += 2) {
      const left = currentLayer[i];
      const right = currentLayer[i + 1] || EMPTY_NODE;
      nextLayer.push({
        hash: hashInternal(left, right),
        sumWeight: left.sumWeight + right.sumWeight,
      });
    }

    coveredIndices = nextCovered;
    currentLayer = nextLayer;
    level++;
  }

  // Sort auxiliary nodes by (level, index) for canonical ordering per spec F.3.1
  auxiliaryNodes.sort((a, b) => {
    if (a.level !== b.level) return a.level - b.level;
    return a.index - b.index;
  });

  return {
    committeeSize: sortedLeaves.length,  // N - included per spec F.3.1
    leaves: signerLeaves,
    auxiliaryNodes,
  };
}

export function verifyMerkleSumMultiProof(
  multiProof: MerkleSumMultiProof,
  expectedRoot: MerkleSumRoot
): { valid: boolean; signerWeight: number; errors: string[] } {
  const errors: string[] = [];

  if (multiProof.leaves.length === 0) {
    errors.push("No leaves in multi-proof");
    return { valid: false, signerWeight: 0, errors };
  }

  // Derive treeDepth from committeeSize per spec F.3.1
  const depth = Math.ceil(Math.log2(multiProof.committeeSize));
  const layers: Map<number, MerkleSumNode>[] = [];
  for (let i = 0; i <= depth; i++) {
    layers.push(new Map());
  }

  const sortedLeaves = [...multiProof.leaves].sort((a, b) => a.index - b.index);
  let signerWeight = 0;
  for (const leaf of sortedLeaves) {
    const positionInLayer = leaf.index;
    layers[0].set(positionInLayer, {
      hash: hashLeaf(leaf),
      sumWeight: leaf.weight,
    });
    signerWeight += leaf.weight;
  }

  for (const aux of multiProof.auxiliaryNodes) {
    layers[aux.level].set(aux.index, aux.node);
  }

  for (let level = 0; level < depth; level++) {
    const currentLayerSize = Math.ceil(multiProof.committeeSize / Math.pow(2, level));

    for (let i = 0; i < currentLayerSize; i += 2) {
      const parentIdx = Math.floor(i / 2);

      if (layers[level + 1].has(parentIdx)) continue;

      const left = layers[level].get(i);
      const right = layers[level].get(i + 1) || EMPTY_NODE;

      if (!left) {
        continue;
      }

      const parent: MerkleSumNode = {
        hash: hashInternal(left, right),
        sumWeight: left.sumWeight + right.sumWeight,
      };

      layers[level + 1].set(parentIdx, parent);
    }
  }

  const computedRoot = layers[depth].get(0);
  if (!computedRoot) {
    errors.push("Failed to compute root from multi-proof");
    return { valid: false, signerWeight, errors };
  }

  if (computedRoot.hash !== expectedRoot.hash) {
    errors.push(
      `Root hash mismatch: computed ${computedRoot.hash.slice(0, 16)}..., expected ${expectedRoot.hash.slice(0, 16)}...`
    );
  }

  if (computedRoot.sumWeight !== expectedRoot.totalWeight) {
    errors.push(
      `Total weight mismatch: computed ${computedRoot.sumWeight}, expected ${expectedRoot.totalWeight}`
    );
  }

  return {
    valid: errors.length === 0,
    signerWeight,
    errors,
  };
}

export function multiProofNodeCount(multiProof: MerkleSumMultiProof): number {
  return multiProof.leaves.length + multiProof.auxiliaryNodes.length;
}

export function individualProofNodeCount(k: number, N: number): number {
  const depth = Math.ceil(Math.log2(N));
  return k * depth;
}
