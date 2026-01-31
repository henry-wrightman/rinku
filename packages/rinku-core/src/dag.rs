use crate::types::DagNode;
use petgraph::graph::{DiGraph, NodeIndex};
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::collections::{HashMap, HashSet, VecDeque};
use thiserror::Error;

/// Maximum number of tips to sample for new transactions (Sparse DAG Sampling)
/// This prevents tip explosion while maintaining DAG connectivity
pub const MAX_SAMPLED_TIPS: usize = 16;

#[derive(Error, Debug)]
pub enum DagError {
    #[error("Node not found: {0}")]
    NodeNotFound(String),
    #[error("Invalid parent reference: {0}")]
    InvalidParent(String),
    #[error("Cycle detected")]
    CycleDetected,
    #[error("DAG capacity exceeded")]
    CapacityExceeded,
}

#[derive(Debug)]
pub struct Dag {
    graph: DiGraph<DagNode, ()>,
    hash_to_index: HashMap<String, NodeIndex>,
    tips: HashSet<String>,
    /// Index of unfinalized transaction hashes for O(1) lookup
    /// This avoids O(n) DAG scan when creating checkpoints
    unfinalized: HashSet<String>,
    max_nodes: usize,
}

impl Dag {
    pub fn new(max_nodes: usize) -> Self {
        Self {
            graph: DiGraph::new(),
            hash_to_index: HashMap::new(),
            tips: HashSet::new(),
            unfinalized: HashSet::new(),
            max_nodes,
        }
    }

    pub fn add_node(&mut self, node: DagNode) -> Result<(), DagError> {
        if self.hash_to_index.contains_key(&node.hash) {
            return Ok(());
        }

        let hash = node.hash.clone();
        let parents = node.parents.clone();

        for parent_hash in &parents {
            if !parent_hash.is_empty() && parent_hash == &hash {
                return Err(DagError::CycleDetected);
            }
        }

        let node_idx = self.graph.add_node(node);
        self.hash_to_index.insert(hash.clone(), node_idx);
        // New transactions are unfinalized by default
        self.unfinalized.insert(hash.clone());

        for parent_hash in &parents {
            if !parent_hash.is_empty() && parent_hash != &hash {
                if let Some(&parent_idx) = self.hash_to_index.get(parent_hash) {
                    self.graph.add_edge(parent_idx, node_idx, ());

                    if let Some(parent_node) = self.graph.node_weight_mut(parent_idx) {
                        if !parent_node.children.contains(&hash) {
                            parent_node.children.push(hash.clone());
                        }
                    }

                    self.tips.remove(parent_hash);
                }
            }
        }

        self.tips.insert(hash);

        if self.graph.node_count() > self.max_nodes {
            self.prune_oldest()?;
        }

        Ok(())
    }

    pub fn add_node_with_stats(&mut self, node: DagNode) -> Result<(usize, usize), DagError> {
        if self.hash_to_index.contains_key(&node.hash) {
            return Ok((0, 0));
        }

        let hash = node.hash.clone();
        let parents = node.parents.clone();

        for parent_hash in &parents {
            if !parent_hash.is_empty() && parent_hash == &hash {
                return Err(DagError::CycleDetected);
            }
        }

        let total_parents = parents.iter().filter(|p| !p.is_empty() && *p != &hash).count();

        let node_idx = self.graph.add_node(node);
        self.hash_to_index.insert(hash.clone(), node_idx);
        // New transactions are unfinalized by default
        self.unfinalized.insert(hash.clone());

        let mut linked_parents = 0;
        for parent_hash in &parents {
            if !parent_hash.is_empty() && parent_hash != &hash {
                if let Some(&parent_idx) = self.hash_to_index.get(parent_hash) {
                    self.graph.add_edge(parent_idx, node_idx, ());

                    if let Some(parent_node) = self.graph.node_weight_mut(parent_idx) {
                        if !parent_node.children.contains(&hash) {
                            parent_node.children.push(hash.clone());
                        }
                    }

                    self.tips.remove(parent_hash);
                    linked_parents += 1;
                }
            }
        }

        self.tips.insert(hash);

        if self.graph.node_count() > self.max_nodes {
            self.prune_oldest()?;
        }

        Ok((linked_parents, total_parents))
    }

    pub fn get_node(&self, hash: &str) -> Option<&DagNode> {
        self.hash_to_index
            .get(hash)
            .and_then(|&idx| self.graph.node_weight(idx))
    }

    pub fn get_node_mut(&mut self, hash: &str) -> Option<&mut DagNode> {
        self.hash_to_index
            .get(hash)
            .copied()
            .and_then(|idx| self.graph.node_weight_mut(idx))
    }

    pub fn contains(&self, hash: &str) -> bool {
        self.hash_to_index.contains_key(hash)
    }

    pub fn tips(&self) -> Vec<String> {
        self.tips.iter().cloned().collect()
    }

    pub fn tip_count(&self) -> usize {
        self.tips.len()
    }

    /// Get a weighted random sample of tips for new transactions (Sparse DAG Sampling)
    /// - Returns at most MAX_SAMPLED_TIPS tips
    /// - Prefers tips with higher node weight (sender's account weight = Sybil resistance)
    /// - If fewer tips than MAX_SAMPLED_TIPS, returns all tips
    /// 
    /// Weight selection: Uses the tip node's inherent weight (from sender's account),
    /// not cumulative descendant weight (which is always 0 for tips since tips have no children).
    pub fn get_sampled_tips(&self) -> Vec<String> {
        let all_tips: Vec<String> = self.tips.iter().cloned().collect();
        
        // If we have fewer tips than the max, return all of them
        if all_tips.len() <= MAX_SAMPLED_TIPS {
            return all_tips;
        }
        
        // Get node weights for all tips (sender's account weight = Sybil resistance)
        // Higher weight = sender has more stake/age = more trustworthy
        let mut weighted_tips: Vec<(String, f64)> = all_tips
            .iter()
            .filter_map(|tip| {
                self.get_node(tip).map(|node| (tip.clone(), node.weight))
            })
            .collect();
        
        // Sort by weight descending (higher weight = more trustworthy sender)
        weighted_tips.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        
        // Take top half by weight, randomly sample from rest for diversity
        let guaranteed_count = MAX_SAMPLED_TIPS / 2;
        let random_count = MAX_SAMPLED_TIPS - guaranteed_count;
        
        let mut result: Vec<String> = weighted_tips
            .iter()
            .take(guaranteed_count)
            .map(|(tip, _)| tip.clone())
            .collect();
        
        // Randomly sample from remaining tips for DAG diversity
        let remaining: Vec<String> = weighted_tips
            .iter()
            .skip(guaranteed_count)
            .map(|(tip, _)| tip.clone())
            .collect();
        
        if !remaining.is_empty() {
            let mut rng = thread_rng();
            let sample_size = random_count.min(remaining.len());
            let mut shuffled = remaining;
            shuffled.shuffle(&mut rng);
            result.extend(shuffled.into_iter().take(sample_size));
        }
        
        result
    }

    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn get_ancestors(&self, hash: &str, max_depth: usize) -> Vec<String> {
        let mut ancestors = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        if let Some(&idx) = self.hash_to_index.get(hash) {
            queue.push_back((idx, 0));
            visited.insert(idx);
        }

        while let Some((current_idx, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }

            for neighbor in self
                .graph
                .neighbors_directed(current_idx, petgraph::Direction::Incoming)
            {
                if visited.insert(neighbor) {
                    if let Some(node) = self.graph.node_weight(neighbor) {
                        ancestors.push(node.hash.clone());
                    }
                    queue.push_back((neighbor, depth + 1));
                }
            }
        }

        ancestors
    }

    pub fn get_descendants(&self, hash: &str, max_depth: usize) -> Vec<String> {
        let mut descendants = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        if let Some(&idx) = self.hash_to_index.get(hash) {
            queue.push_back((idx, 0));
            visited.insert(idx);
        }

        while let Some((current_idx, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }

            for neighbor in self
                .graph
                .neighbors_directed(current_idx, petgraph::Direction::Outgoing)
            {
                if visited.insert(neighbor) {
                    if let Some(node) = self.graph.node_weight(neighbor) {
                        descendants.push(node.hash.clone());
                    }
                    queue.push_back((neighbor, depth + 1));
                }
            }
        }

        descendants
    }

    pub fn get_all_nodes(&self) -> Vec<&DagNode> {
        self.graph.node_weights().collect()
    }
    
    /// Get mutable iterator over all nodes (used for recalculating weights)
    pub fn nodes_mut(&mut self) -> impl Iterator<Item = &mut DagNode> {
        self.graph.node_weights_mut()
    }

    pub fn all_transactions(&self) -> Vec<crate::types::SignedTransaction> {
        self.graph.node_weights().map(|n| n.tx.clone()).collect()
    }

    /// Get unfinalized nodes using the O(1) index instead of O(n) scan
    pub fn get_unfinalized_nodes(&self) -> Vec<&DagNode> {
        self.unfinalized
            .iter()
            .filter_map(|hash| self.get_node(hash))
            .collect()
    }
    
    /// Get count of unfinalized transactions (O(1))
    pub fn unfinalized_count(&self) -> usize {
        self.unfinalized.len()
    }

    pub fn mark_finalized(&mut self, hash: &str, checkpoint_height: u64) -> Result<(), DagError> {
        if let Some(node) = self.get_node_mut(hash) {
            node.finalized = true;
            node.checkpoint_height = Some(checkpoint_height);
            // Remove from unfinalized index for O(1) checkpoint creation
            self.unfinalized.remove(hash);
            Ok(())
        } else {
            Err(DagError::NodeNotFound(hash.to_string()))
        }
    }

    /// Batch mark multiple transactions as finalized (optimized for checkpoint creation)
    /// Returns the number of transactions successfully marked
    pub fn mark_finalized_batch(&mut self, hashes: &[String], checkpoint_height: u64) -> usize {
        let mut count = 0;
        for hash in hashes {
            if let Some(node) = self.get_node_mut(hash) {
                node.finalized = true;
                node.checkpoint_height = Some(checkpoint_height);
                count += 1;
            }
        }
        // Bulk remove from unfinalized set (more efficient than individual removes)
        for hash in hashes {
            self.unfinalized.remove(hash);
        }
        count
    }

    /// Unfinalize a transaction (used during checkpoint chain recovery)
    pub fn unfinalize(&mut self, hash: &str) {
        if let Some(node) = self.get_node_mut(hash) {
            node.finalized = false;
            node.checkpoint_height = None;
            // Add back to unfinalized index
            self.unfinalized.insert(hash.to_string());
        }
    }

    fn prune_oldest(&mut self) -> Result<(), DagError> {
        let target_size = self.max_nodes * 3 / 4;

        let mut finalized_with_time: Vec<(NodeIndex, u64)> = self
            .graph
            .node_indices()
            .filter_map(|idx| {
                self.graph.node_weight(idx).and_then(|node| {
                    if node.finalized && !self.tips.contains(&node.hash) {
                        Some((idx, node.tx.tx.timestamp))
                    } else {
                        None
                    }
                })
            })
            .collect();

        finalized_with_time.sort_by_key(|(_, ts)| *ts);

        let to_remove_count = self.graph.node_count().saturating_sub(target_size);
        let to_remove: Vec<NodeIndex> = finalized_with_time
            .into_iter()
            .take(to_remove_count)
            .map(|(idx, _)| idx)
            .collect();

        for idx in to_remove.into_iter().rev() {
            if let Some(node) = self.graph.node_weight(idx) {
                let hash = node.hash.clone();
                self.hash_to_index.remove(&hash);
                self.tips.remove(&hash);
                self.unfinalized.remove(&hash);
            }
            self.graph.remove_node(idx);
        }

        self.rebuild_index();

        Ok(())
    }

    fn rebuild_index(&mut self) {
        self.hash_to_index.clear();
        self.unfinalized.clear();
        for idx in self.graph.node_indices() {
            if let Some(node) = self.graph.node_weight(idx) {
                self.hash_to_index.insert(node.hash.clone(), idx);
                if !node.finalized {
                    self.unfinalized.insert(node.hash.clone());
                }
            }
        }
    }

    pub fn calculate_cumulative_weight(&self, hash: &str) -> f64 {
        let descendants = self.get_descendants(hash, usize::MAX);

        let mut total_weight = 0.0;

        if let Some(node) = self.get_node(hash) {
            total_weight += node.weight;
        }

        for desc_hash in descendants {
            if let Some(node) = self.get_node(&desc_hash) {
                total_weight += node.weight;
            }
        }

        total_weight
    }

    pub fn remove_node(&mut self, hash: &str) -> Option<DagNode> {
        let idx = self.hash_to_index.remove(hash)?;
        self.tips.remove(hash);
        self.unfinalized.remove(hash);

        let parent_hashes: Vec<String> = self.graph
            .node_weight(idx)
            .map(|n| n.parents.clone())
            .unwrap_or_default();

        for parent_hash in parent_hashes {
            if let Some(&parent_idx) = self.hash_to_index.get(&parent_hash) {
                if let Some(parent_node) = self.graph.node_weight_mut(parent_idx) {
                    parent_node.children.retain(|c| c != hash);
                    if parent_node.children.is_empty() {
                        self.tips.insert(parent_hash.clone());
                    }
                }
            }
        }

        let removed = self.graph.remove_node(idx);
        
        self.rebuild_index();
        
        removed
    }

    pub fn prune_branch(&mut self, root_hash: &str) -> Vec<DagNode> {
        let descendants = self.get_descendants(root_hash, usize::MAX);
        
        let mut to_remove: Vec<String> = descendants;
        to_remove.push(root_hash.to_string());

        to_remove.sort_by(|a, b| {
            let depth_a = self.get_ancestors(a, usize::MAX).len();
            let depth_b = self.get_ancestors(b, usize::MAX).len();
            depth_b.cmp(&depth_a)
        });

        let non_finalized: Vec<String> = to_remove
            .into_iter()
            .filter(|hash| {
                if let Some(idx) = self.hash_to_index.get(hash) {
                    if let Some(node) = self.graph.node_weight(*idx) {
                        return !node.finalized;
                    }
                }
                false
            })
            .collect();

        let mut removed_nodes = Vec::new();
        for hash in non_finalized {
            if let Some(node) = self.remove_node(&hash) {
                removed_nodes.push(node);
            }
        }

        removed_nodes
    }

    pub fn is_ancestor(&self, ancestor_hash: &str, descendant_hash: &str) -> bool {
        let ancestors = self.get_ancestors(descendant_hash, usize::MAX);
        ancestors.contains(&ancestor_hash.to_string())
    }

    pub fn find_common_ancestor(&self, hash_a: &str, hash_b: &str) -> Option<String> {
        let ancestors_a: HashSet<String> = self.get_ancestors(hash_a, usize::MAX).into_iter().collect();
        let ancestors_b = self.get_ancestors(hash_b, usize::MAX);

        for ancestor in ancestors_b {
            if ancestors_a.contains(&ancestor) {
                return Some(ancestor);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Transaction, SignedTransaction};

    fn make_tx(hash: &str, parents: Vec<String>) -> DagNode {
        let tx = Transaction {
            from: "sender".to_string(),
            to: "receiver".to_string(),
            amount: 1.0,
            nonce: 0,
            timestamp: 1000,
            parents: parents.clone(),
            kind: None,
            gas_limit: None,
            gas_price: None,
            data: None,
            signature: None,
            memo: None,
            references: None,
        };

        let signed = SignedTransaction {
            tx,
            hash: hash.to_string(),
            signature: "sig".to_string(),
        };

        DagNode {
            hash: hash.to_string(),
            tx: signed,
            parents,
            children: Vec::new(),
            weight: 1.0,
            finalized: false,
            checkpoint_height: None,
            received_at_ms: Some(0),
        }
    }

    #[test]
    fn test_add_and_get_node() {
        let mut dag = Dag::new(100);
        let node = make_tx("hash1", vec![]);
        dag.add_node(node).unwrap();

        assert!(dag.contains("hash1"));
        assert_eq!(dag.node_count(), 1);
        assert_eq!(dag.tip_count(), 1);
    }

    #[test]
    fn test_parent_child_relationship() {
        let mut dag = Dag::new(100);

        let parent = make_tx("parent", vec![]);
        dag.add_node(parent).unwrap();

        let child = make_tx("child", vec!["parent".to_string()]);
        dag.add_node(child).unwrap();

        assert_eq!(dag.tip_count(), 1);
        assert!(dag.tips().contains(&"child".to_string()));

        let parent_node = dag.get_node("parent").unwrap();
        assert!(parent_node.children.contains(&"child".to_string()));
    }

    #[test]
    fn test_ancestors() {
        let mut dag = Dag::new(100);

        dag.add_node(make_tx("a", vec![])).unwrap();
        dag.add_node(make_tx("b", vec!["a".to_string()])).unwrap();
        dag.add_node(make_tx("c", vec!["b".to_string()])).unwrap();

        let ancestors = dag.get_ancestors("c", 10);
        assert_eq!(ancestors.len(), 2);
        assert!(ancestors.contains(&"a".to_string()));
        assert!(ancestors.contains(&"b".to_string()));
    }

    #[test]
    fn test_descendants() {
        let mut dag = Dag::new(100);

        dag.add_node(make_tx("a", vec![])).unwrap();
        dag.add_node(make_tx("b", vec!["a".to_string()])).unwrap();
        dag.add_node(make_tx("c", vec!["a".to_string()])).unwrap();

        let descendants = dag.get_descendants("a", 10);
        assert_eq!(descendants.len(), 2);
        assert!(descendants.contains(&"b".to_string()));
        assert!(descendants.contains(&"c".to_string()));
    }

    #[test]
    fn test_remove_node() {
        let mut dag = Dag::new(100);

        dag.add_node(make_tx("a", vec![])).unwrap();
        dag.add_node(make_tx("b", vec!["a".to_string()])).unwrap();

        assert_eq!(dag.node_count(), 2);
        assert!(dag.tips().contains(&"b".to_string()));

        let removed = dag.remove_node("b");
        assert!(removed.is_some());
        assert_eq!(dag.node_count(), 1);
        assert!(dag.tips().contains(&"a".to_string()));
        assert!(!dag.contains("b"));
    }

    #[test]
    fn test_prune_branch() {
        let mut dag = Dag::new(100);

        dag.add_node(make_tx("root", vec![])).unwrap();
        dag.add_node(make_tx("a", vec!["root".to_string()])).unwrap();
        dag.add_node(make_tx("b", vec!["root".to_string()])).unwrap();
        dag.add_node(make_tx("a1", vec!["a".to_string()])).unwrap();
        dag.add_node(make_tx("a2", vec!["a".to_string()])).unwrap();

        assert_eq!(dag.node_count(), 5);

        let removed = dag.prune_branch("a");
        assert_eq!(removed.len(), 3);
        assert_eq!(dag.node_count(), 2);
        assert!(dag.contains("root"));
        assert!(dag.contains("b"));
        assert!(!dag.contains("a"));
        assert!(!dag.contains("a1"));
        assert!(!dag.contains("a2"));
    }

    #[test]
    fn test_prune_branch_protects_finalized() {
        let mut dag = Dag::new(100);

        dag.add_node(make_tx("root", vec![])).unwrap();
        dag.add_node(make_tx("a", vec!["root".to_string()])).unwrap();
        dag.add_node(make_tx("a1", vec!["a".to_string()])).unwrap();
        dag.add_node(make_tx("a2", vec!["a1".to_string()])).unwrap();

        if let Some(idx) = dag.hash_to_index.get("a1").copied() {
            if let Some(node) = dag.graph.node_weight_mut(idx) {
                node.finalized = true;
            }
        }

        assert_eq!(dag.node_count(), 4);

        let removed = dag.prune_branch("a");
        
        assert_eq!(removed.len(), 2);
        assert!(dag.contains("root"));
        assert!(!dag.contains("a"));
        assert!(dag.contains("a1"));
        assert!(!dag.contains("a2"));
        assert_eq!(dag.node_count(), 2);
    }

    #[test]
    fn test_cumulative_weight() {
        let mut dag = Dag::new(100);

        dag.add_node(make_tx("a", vec![])).unwrap();
        dag.add_node(make_tx("b", vec!["a".to_string()])).unwrap();
        dag.add_node(make_tx("c", vec!["b".to_string()])).unwrap();

        let weight = dag.calculate_cumulative_weight("a");
        assert_eq!(weight, 3.0);

        let weight_b = dag.calculate_cumulative_weight("b");
        assert_eq!(weight_b, 2.0);

        let weight_c = dag.calculate_cumulative_weight("c");
        assert_eq!(weight_c, 1.0);
    }

    #[test]
    fn test_find_common_ancestor() {
        let mut dag = Dag::new(100);

        dag.add_node(make_tx("root", vec![])).unwrap();
        dag.add_node(make_tx("a", vec!["root".to_string()])).unwrap();
        dag.add_node(make_tx("b", vec!["root".to_string()])).unwrap();
        dag.add_node(make_tx("a1", vec!["a".to_string()])).unwrap();
        dag.add_node(make_tx("b1", vec!["b".to_string()])).unwrap();

        let common = dag.find_common_ancestor("a1", "b1");
        assert_eq!(common, Some("root".to_string()));
    }

    #[test]
    fn test_missing_parents_accepted() {
        let mut dag = Dag::new(100);

        dag.add_node(make_tx("root", vec![])).unwrap();

        let result = dag.add_node(make_tx("child", vec![
            "root".to_string(),
            "missing_parent".to_string(),
        ]));
        assert!(result.is_ok());

        assert!(dag.contains("child"));
        assert_eq!(dag.tip_count(), 1);
        assert!(dag.tips().contains(&"child".to_string()));

        let root_node = dag.get_node("root").unwrap();
        assert!(root_node.children.contains(&"child".to_string()));
    }

    #[test]
    fn test_add_node_with_stats_tracks_linked_parents() {
        let mut dag = Dag::new(100);

        dag.add_node(make_tx("root", vec![])).unwrap();

        let (linked, total) = dag.add_node_with_stats(make_tx("child", vec![
            "root".to_string(),
            "missing1".to_string(),
            "missing2".to_string(),
        ])).unwrap();

        assert_eq!(linked, 1);
        assert_eq!(total, 3);
    }

    #[test]
    fn test_self_parent_rejected() {
        let mut dag = Dag::new(100);

        let result = dag.add_node(make_tx("self_ref", vec!["self_ref".to_string()]));
        assert!(result.is_err());
        
        match result {
            Err(DagError::CycleDetected) => {}
            _ => panic!("Expected CycleDetected error"),
        }

        assert!(!dag.contains("self_ref"));
    }
}
