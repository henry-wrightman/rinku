use crate::crypto::sha256;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MerkleError {
    #[error("Empty leaf list")]
    EmptyLeaves,
    #[error("Invalid proof")]
    InvalidProof,
    #[error("Leaf not found in tree")]
    LeafNotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleProof {
    pub leaf_hash: String,
    pub siblings: Vec<String>,
    pub path_bits: Vec<bool>,
    pub root: String,
}

#[derive(Debug, Clone)]
pub struct MerkleTree {
    leaves: Vec<[u8; 32]>,
    layers: Vec<Vec<[u8; 32]>>,
    root: [u8; 32],
}

impl MerkleTree {
    pub fn new(leaf_hashes: Vec<[u8; 32]>) -> Result<Self, MerkleError> {
        if leaf_hashes.is_empty() {
            return Err(MerkleError::EmptyLeaves);
        }

        let mut layers: Vec<Vec<[u8; 32]>> = Vec::new();
        let mut current_layer = leaf_hashes.clone();

        while current_layer.len() > 1 {
            layers.push(current_layer.clone());
            current_layer = Self::compute_next_layer(&current_layer);
        }
        layers.push(current_layer.clone());

        let root = current_layer[0];

        Ok(Self {
            leaves: leaf_hashes,
            layers,
            root,
        })
    }

    pub fn from_hex_leaves(hex_leaves: &[String]) -> Result<Self, MerkleError> {
        let leaves: Vec<[u8; 32]> = hex_leaves
            .iter()
            .map(|h| {
                let bytes = hex::decode(h).unwrap_or_default();
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes[..32.min(bytes.len())]);
                arr
            })
            .collect();
        Self::new(leaves)
    }

    fn compute_next_layer(current: &[[u8; 32]]) -> Vec<[u8; 32]> {
        let mut next = Vec::new();
        let mut i = 0;

        while i < current.len() {
            let left = current[i];
            let right = if i + 1 < current.len() {
                current[i + 1]
            } else {
                current[i]
            };

            let mut combined = Vec::with_capacity(64);
            combined.extend_from_slice(&left);
            combined.extend_from_slice(&right);
            next.push(sha256(&combined));

            i += 2;
        }

        next
    }

    pub fn root(&self) -> String {
        hex::encode(self.root)
    }

    pub fn root_bytes(&self) -> [u8; 32] {
        self.root
    }

    pub fn get_proof(&self, leaf_index: usize) -> Result<MerkleProof, MerkleError> {
        if leaf_index >= self.leaves.len() {
            return Err(MerkleError::LeafNotFound);
        }

        let mut siblings = Vec::new();
        let mut path_bits = Vec::new();
        let mut current_index = leaf_index;

        for layer in &self.layers[..self.layers.len() - 1] {
            let is_right = current_index % 2 == 1;
            path_bits.push(is_right);

            let sibling_index = if is_right {
                current_index - 1
            } else {
                if current_index + 1 < layer.len() {
                    current_index + 1
                } else {
                    current_index
                }
            };

            siblings.push(hex::encode(layer[sibling_index]));
            current_index /= 2;
        }

        Ok(MerkleProof {
            leaf_hash: hex::encode(self.leaves[leaf_index]),
            siblings,
            path_bits,
            root: self.root(),
        })
    }

    pub fn get_proof_by_hash(&self, leaf_hash: &str) -> Result<MerkleProof, MerkleError> {
        let hash_bytes = hex::decode(leaf_hash).map_err(|_| MerkleError::InvalidProof)?;
        let mut target = [0u8; 32];
        target.copy_from_slice(&hash_bytes[..32.min(hash_bytes.len())]);

        for (i, leaf) in self.leaves.iter().enumerate() {
            if leaf == &target {
                return self.get_proof(i);
            }
        }

        Err(MerkleError::LeafNotFound)
    }
}

pub fn verify_proof(proof: &MerkleProof) -> Result<bool, MerkleError> {
    let mut current_hash =
        hex::decode(&proof.leaf_hash).map_err(|_| MerkleError::InvalidProof)?;

    for (i, sibling_hex) in proof.siblings.iter().enumerate() {
        let sibling = hex::decode(sibling_hex).map_err(|_| MerkleError::InvalidProof)?;
        let is_right = proof.path_bits.get(i).copied().unwrap_or(false);

        let mut combined = Vec::with_capacity(64);
        if is_right {
            combined.extend_from_slice(&sibling);
            combined.extend_from_slice(&current_hash);
        } else {
            combined.extend_from_slice(&current_hash);
            combined.extend_from_slice(&sibling);
        }

        current_hash = sha256(&combined).to_vec();
    }

    let computed_root = hex::encode(&current_hash);
    Ok(computed_root == proof.root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::sha256_hex;

    #[test]
    fn test_merkle_tree_single_leaf() {
        let leaves = vec![sha256(b"leaf1")];
        let tree = MerkleTree::new(leaves).unwrap();
        assert!(!tree.root().is_empty());
    }

    #[test]
    fn test_merkle_tree_multiple_leaves() {
        let leaves = vec![
            sha256(b"leaf1"),
            sha256(b"leaf2"),
            sha256(b"leaf3"),
            sha256(b"leaf4"),
        ];
        let tree = MerkleTree::new(leaves).unwrap();

        let proof = tree.get_proof(0).unwrap();
        assert!(verify_proof(&proof).unwrap());

        let proof = tree.get_proof(2).unwrap();
        assert!(verify_proof(&proof).unwrap());
    }

    #[test]
    fn test_merkle_tree_odd_leaves() {
        let leaves = vec![sha256(b"leaf1"), sha256(b"leaf2"), sha256(b"leaf3")];
        let tree = MerkleTree::new(leaves).unwrap();

        for i in 0..3 {
            let proof = tree.get_proof(i).unwrap();
            assert!(verify_proof(&proof).unwrap());
        }
    }

    #[test]
    fn test_proof_by_hash() {
        let leaves: Vec<[u8; 32]> = vec![sha256(b"tx1"), sha256(b"tx2"), sha256(b"tx3")];
        let tree = MerkleTree::new(leaves.clone()).unwrap();

        let leaf_hash = hex::encode(leaves[1]);
        let proof = tree.get_proof_by_hash(&leaf_hash).unwrap();
        assert!(verify_proof(&proof).unwrap());
    }
}
