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
      const right = currentLayer[i + 1] || { hash: "padding", sumWeight: 0 };

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
        : { hash: "padding", sumWeight: 0 };

    siblings.push(sibling);
    pathBits.push(isRight);

    const nextLayer: MerkleSumNode[] = [];
    for (let i = 0; i < currentLayer.length; i += 2) {
      const left = currentLayer[i];
      const right = currentLayer[i + 1] || { hash: "padding", sumWeight: 0 };
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
