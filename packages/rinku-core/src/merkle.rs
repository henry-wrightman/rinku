use crate::crypto::sha256;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleMultiProof {
    pub leaf_hashes: Vec<String>,
    pub leaf_indices: Vec<usize>,
    pub helper_hashes: Vec<String>,
    pub helper_indices: Vec<(usize, usize)>,
    pub num_leaves: usize,
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
        let mut leaves: Vec<[u8; 32]> = Vec::with_capacity(hex_leaves.len());
        
        for hex_str in hex_leaves {
            let bytes = hex::decode(hex_str).map_err(|_| MerkleError::InvalidProof)?;
            if bytes.len() != 32 {
                return Err(MerkleError::InvalidProof);
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            leaves.push(arr);
        }
        
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

    pub fn get_multiproof(&self, leaf_indices: &[usize]) -> Result<MerkleMultiProof, MerkleError> {
        if leaf_indices.is_empty() {
            return Err(MerkleError::EmptyLeaves);
        }

        for &idx in leaf_indices {
            if idx >= self.leaves.len() {
                return Err(MerkleError::LeafNotFound);
            }
        }

        let mut sorted_indices: Vec<usize> = leaf_indices.to_vec();
        sorted_indices.sort();
        sorted_indices.dedup();

        let leaf_hashes: Vec<String> = sorted_indices
            .iter()
            .map(|&i| hex::encode(self.leaves[i]))
            .collect();

        let mut helper_hashes = Vec::new();
        let mut helper_indices = Vec::new();

        let mut known_positions: HashSet<usize> = sorted_indices.iter().copied().collect();

        for (layer_idx, layer) in self.layers.iter().enumerate() {
            if layer_idx >= self.layers.len() - 1 {
                break;
            }

            let mut next_known = HashSet::new();
            let positions: Vec<usize> = known_positions.iter().copied().collect();

            for &pos in &positions {
                let sibling = if pos % 2 == 0 {
                    if pos + 1 < layer.len() {
                        pos + 1
                    } else {
                        pos
                    }
                } else {
                    pos - 1
                };

                if sibling != pos && !known_positions.contains(&sibling) {
                    helper_hashes.push(hex::encode(layer[sibling]));
                    helper_indices.push((layer_idx, sibling));
                }

                next_known.insert(pos / 2);
            }

            known_positions = next_known;
        }

        Ok(MerkleMultiProof {
            leaf_hashes,
            leaf_indices: sorted_indices,
            helper_hashes,
            helper_indices,
            num_leaves: self.leaves.len(),
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

pub fn verify_multiproof(proof: &MerkleMultiProof) -> Result<bool, MerkleError> {
    if proof.leaf_hashes.len() != proof.leaf_indices.len() {
        return Err(MerkleError::InvalidProof);
    }
    if proof.helper_hashes.len() != proof.helper_indices.len() {
        return Err(MerkleError::InvalidProof);
    }
    if proof.num_leaves == 0 {
        return Err(MerkleError::InvalidProof);
    }

    let mut known: HashMap<(usize, usize), [u8; 32]> = HashMap::new();

    for (i, leaf_hex) in proof.leaf_hashes.iter().enumerate() {
        let bytes = hex::decode(leaf_hex).map_err(|_| MerkleError::InvalidProof)?;
        if bytes.len() != 32 {
            return Err(MerkleError::InvalidProof);
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        known.insert((0, proof.leaf_indices[i]), arr);
    }

    for (i, helper_hex) in proof.helper_hashes.iter().enumerate() {
        let bytes = hex::decode(helper_hex).map_err(|_| MerkleError::InvalidProof)?;
        if bytes.len() != 32 {
            return Err(MerkleError::InvalidProof);
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        known.insert(proof.helper_indices[i], arr);
    }

    let mut layer_sizes = Vec::new();
    let mut size = proof.num_leaves;
    while size > 1 {
        layer_sizes.push(size);
        size = (size + 1) / 2;
    }
    layer_sizes.push(1);

    let num_layers = layer_sizes.len();

    for layer_idx in 0..num_layers - 1 {
        let layer_size = layer_sizes[layer_idx];
        let positions: Vec<usize> = known
            .keys()
            .filter(|(l, _)| *l == layer_idx)
            .map(|(_, p)| *p)
            .collect();

        let mut processed = HashSet::new();
        for pos in positions {
            let left_pos = if pos % 2 == 0 { pos } else { pos - 1 };
            if processed.contains(&left_pos) {
                continue;
            }
            processed.insert(left_pos);

            let right_pos = if left_pos + 1 < layer_size {
                left_pos + 1
            } else {
                left_pos
            };

            if let (Some(&left), Some(&right)) = (
                known.get(&(layer_idx, left_pos)),
                known.get(&(layer_idx, right_pos)),
            ) {
                let mut combined = Vec::with_capacity(64);
                combined.extend_from_slice(&left);
                combined.extend_from_slice(&right);
                let parent = sha256(&combined);
                known.insert((layer_idx + 1, left_pos / 2), parent);
            }
        }
    }

    if let Some(computed_root) = known.get(&(num_layers - 1, 0)) {
        Ok(hex::encode(computed_root) == proof.root)
    } else {
        Ok(false)
    }
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

    #[test]
    fn test_from_hex_leaves_invalid_hex() {
        let result = MerkleTree::from_hex_leaves(&["not_valid_hex".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_hex_leaves_wrong_length() {
        let result = MerkleTree::from_hex_leaves(&["aabbcc".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_hex_leaves_valid() {
        let hash = hex::encode(sha256(b"test"));
        let result = MerkleTree::from_hex_leaves(&[hash]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multiproof_two_of_four_leaves() {
        let leaves = vec![
            sha256(b"leaf0"),
            sha256(b"leaf1"),
            sha256(b"leaf2"),
            sha256(b"leaf3"),
        ];
        let tree = MerkleTree::new(leaves).unwrap();

        let proof = tree.get_multiproof(&[0, 2]).unwrap();
        assert_eq!(proof.leaf_hashes.len(), 2);
        assert_eq!(proof.leaf_indices, vec![0, 2]);
        assert!(verify_multiproof(&proof).unwrap());
    }

    #[test]
    fn test_multiproof_all_leaves() {
        let leaves = vec![
            sha256(b"leaf0"),
            sha256(b"leaf1"),
            sha256(b"leaf2"),
            sha256(b"leaf3"),
        ];
        let tree = MerkleTree::new(leaves).unwrap();

        let proof = tree.get_multiproof(&[0, 1, 2, 3]).unwrap();
        assert_eq!(proof.leaf_hashes.len(), 4);
        assert_eq!(proof.helper_hashes.len(), 0);
        assert!(verify_multiproof(&proof).unwrap());
    }

    #[test]
    fn test_multiproof_single_leaf_degenerates() {
        let leaves = vec![
            sha256(b"leaf0"),
            sha256(b"leaf1"),
            sha256(b"leaf2"),
            sha256(b"leaf3"),
        ];
        let tree = MerkleTree::new(leaves).unwrap();

        let proof = tree.get_multiproof(&[1]).unwrap();
        assert_eq!(proof.leaf_hashes.len(), 1);
        assert_eq!(proof.leaf_indices, vec![1]);
        assert!(verify_multiproof(&proof).unwrap());
    }

    #[test]
    fn test_multiproof_odd_leaf_count() {
        let leaves = vec![
            sha256(b"leaf0"),
            sha256(b"leaf1"),
            sha256(b"leaf2"),
            sha256(b"leaf3"),
            sha256(b"leaf4"),
        ];
        let tree = MerkleTree::new(leaves).unwrap();

        let proof = tree.get_multiproof(&[1, 3]).unwrap();
        assert!(verify_multiproof(&proof).unwrap());

        let proof_with_last = tree.get_multiproof(&[0, 4]).unwrap();
        assert!(verify_multiproof(&proof_with_last).unwrap());

        let proof_all = tree.get_multiproof(&[0, 1, 2, 3, 4]).unwrap();
        assert_eq!(proof_all.helper_hashes.len(), 0);
        assert!(verify_multiproof(&proof_all).unwrap());
    }

    #[test]
    fn test_multiproof_adjacent_leaves() {
        let leaves = vec![
            sha256(b"leaf0"),
            sha256(b"leaf1"),
            sha256(b"leaf2"),
            sha256(b"leaf3"),
        ];
        let tree = MerkleTree::new(leaves).unwrap();

        let proof = tree.get_multiproof(&[0, 1]).unwrap();
        assert_eq!(proof.leaf_hashes.len(), 2);
        assert!(verify_multiproof(&proof).unwrap());
    }

    #[test]
    fn test_multiproof_invalid_index() {
        let leaves = vec![sha256(b"leaf0"), sha256(b"leaf1")];
        let tree = MerkleTree::new(leaves).unwrap();

        assert!(tree.get_multiproof(&[5]).is_err());
    }

    #[test]
    fn test_multiproof_empty_indices() {
        let leaves = vec![sha256(b"leaf0"), sha256(b"leaf1")];
        let tree = MerkleTree::new(leaves).unwrap();

        assert!(tree.get_multiproof(&[]).is_err());
    }

    #[test]
    fn test_multiproof_tampered_root_fails() {
        let leaves = vec![
            sha256(b"leaf0"),
            sha256(b"leaf1"),
            sha256(b"leaf2"),
            sha256(b"leaf3"),
        ];
        let tree = MerkleTree::new(leaves).unwrap();

        let mut proof = tree.get_multiproof(&[0, 2]).unwrap();
        proof.root = hex::encode([0u8; 32]);
        assert!(!verify_multiproof(&proof).unwrap());
    }
}
