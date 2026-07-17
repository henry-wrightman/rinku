use crate::types::DagNode;
use petgraph::graph::{DiGraph, NodeIndex};
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
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
    unfinalized: HashSet<String>,
    sender_unfinalized: HashMap<String, HashSet<String>>,
    finalized_by_height: BTreeMap<u64, HashSet<String>>,
    max_nodes: usize,
}

impl Dag {
    pub fn new(max_nodes: usize) -> Self {
        Self {
            graph: DiGraph::new(),
            hash_to_index: HashMap::new(),
            tips: HashSet::new(),
            unfinalized: HashSet::new(),
            sender_unfinalized: HashMap::new(),
            finalized_by_height: BTreeMap::new(),
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

        let is_finalized = node.finalized;
        let checkpoint_height = node.checkpoint_height;
        let sender = node.tx.tx.from.clone();
        let node_idx = self.graph.add_node(node);
        self.hash_to_index.insert(hash.clone(), node_idx);
        if !is_finalized {
            self.unfinalized.insert(hash.clone());
            self.sender_unfinalized
                .entry(sender)
                .or_default()
                .insert(hash.clone());
        } else if let Some(h) = checkpoint_height {
            self.finalized_by_height
                .entry(h)
                .or_default()
                .insert(hash.clone());
        }

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

        let total_parents = parents
            .iter()
            .filter(|p| !p.is_empty() && *p != &hash)
            .count();

        let is_finalized = node.finalized;
        let checkpoint_height = node.checkpoint_height;
        let sender = node.tx.tx.from.clone();
        let node_idx = self.graph.add_node(node);
        self.hash_to_index.insert(hash.clone(), node_idx);
        if !is_finalized {
            self.unfinalized.insert(hash.clone());
            self.sender_unfinalized
                .entry(sender)
                .or_default()
                .insert(hash.clone());
        } else if let Some(h) = checkpoint_height {
            self.finalized_by_height
                .entry(h)
                .or_default()
                .insert(hash.clone());
        }

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
            .filter_map(|tip| self.get_node(tip).map(|node| (tip.clone(), node.weight)))
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

    pub fn has_finalized_before(&self, min_checkpoint_height: u64) -> bool {
        if let Some((&lowest_h, _)) = self.finalized_by_height.iter().next() {
            lowest_h < min_checkpoint_height
        } else {
            false
        }
    }

    /// Get immutable iterator over all nodes
    pub fn nodes(&self) -> impl Iterator<Item = &DagNode> {
        self.graph.node_weights()
    }

    /// Get mutable iterator over all nodes (used for recalculating weights)
    pub fn nodes_mut(&mut self) -> impl Iterator<Item = &mut DagNode> {
        self.graph.node_weights_mut()
    }

    /// Rebuild parent-child relationships, graph edges, and tips set after loading from snapshot.
    /// When transactions are loaded in arbitrary order (not topological), parent-child
    /// links may be broken because children are added before their parents exist.
    /// This method iterates all nodes and:
    /// 1. Clears all graph edges and children vectors
    /// 2. Rebuilds graph edges based on parent references
    /// 3. Rebuilds the children vectors for each node
    /// 4. Rebuilds the tips set (nodes with no children)
    ///
    /// Returns (nodes_processed, tips_count, dangling_parents) for logging.
    pub fn rebuild_tips(&mut self) -> (usize, usize, usize) {
        let all_hashes: Vec<String> = self.hash_to_index.keys().cloned().collect();

        // Clear all existing graph edges
        self.graph.clear_edges();

        // Clear existing children vectors
        for hash in &all_hashes {
            if let Some(node) = self.get_node_mut(hash) {
                node.children.clear();
            }
        }

        // Rebuild graph edges and children by iterating all nodes
        let mut dangling_parents = 0usize;
        for hash in &all_hashes {
            let (parents, node_idx) = {
                let node = match self.get_node(hash) {
                    Some(n) => n,
                    None => continue,
                };
                let idx = match self.hash_to_index.get(hash) {
                    Some(&i) => i,
                    None => continue,
                };
                (node.parents.clone(), idx)
            };

            for parent_hash in parents {
                if !parent_hash.is_empty() && parent_hash != *hash {
                    if let Some(&parent_idx) = self.hash_to_index.get(&parent_hash) {
                        // Rebuild graph edge (parent -> child)
                        self.graph.add_edge(parent_idx, node_idx, ());

                        // Rebuild children vector
                        if let Some(parent_node) = self.get_node_mut(&parent_hash) {
                            if !parent_node.children.contains(hash) {
                                parent_node.children.push(hash.clone());
                            }
                        }
                    } else {
                        // Parent hash not found in snapshot - dangling reference
                        dangling_parents += 1;
                    }
                }
            }
        }

        // Rebuild tips: nodes with no children are tips
        self.tips.clear();
        for hash in &all_hashes {
            if let Some(node) = self.get_node(hash) {
                if node.children.is_empty() {
                    self.tips.insert(hash.clone());
                }
            }
        }

        (all_hashes.len(), self.tips.len(), dangling_parents)
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

    /// Get unfinalized nodes for a specific sender using per-sender index
    /// O(K_sender) instead of O(N_total_unfinalized) — critical for add_transaction validation
    pub fn get_unfinalized_for_sender(&self, sender: &str) -> Vec<&DagNode> {
        if let Some(hashes) = self.sender_unfinalized.get(sender) {
            hashes
                .iter()
                .filter_map(|hash| self.get_node(hash))
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get count of unfinalized transactions (O(1))
    pub fn unfinalized_count(&self) -> usize {
        self.unfinalized.len()
    }

    pub fn mark_finalized(&mut self, hash: &str, checkpoint_height: u64) -> Result<(), DagError> {
        if let Some(node) = self.get_node_mut(hash) {
            let sender = node.tx.tx.from.clone();
            let old_cp_h = if node.finalized {
                node.checkpoint_height
            } else {
                None
            };
            node.finalized = true;
            node.checkpoint_height = Some(checkpoint_height);
            if let Some(old_h) = old_cp_h {
                if old_h != checkpoint_height {
                    if let Some(set) = self.finalized_by_height.get_mut(&old_h) {
                        set.remove(hash);
                        if set.is_empty() {
                            self.finalized_by_height.remove(&old_h);
                        }
                    }
                }
            }
            self.unfinalized.remove(hash);
            self.finalized_by_height
                .entry(checkpoint_height)
                .or_default()
                .insert(hash.to_string());
            if let Some(set) = self.sender_unfinalized.get_mut(&sender) {
                set.remove(hash);
                if set.is_empty() {
                    self.sender_unfinalized.remove(&sender);
                }
            }
            Ok(())
        } else {
            Err(DagError::NodeNotFound(hash.to_string()))
        }
    }

    pub fn mark_finalized_deferred_cleanup(
        &mut self,
        hash: &str,
        checkpoint_height: u64,
    ) -> Result<(), DagError> {
        if let Some(node) = self.get_node_mut(hash) {
            let old_cp_h = if node.finalized {
                node.checkpoint_height
            } else {
                None
            };
            node.finalized = true;
            node.checkpoint_height = Some(checkpoint_height);
            if let Some(old_h) = old_cp_h {
                if old_h != checkpoint_height {
                    if let Some(set) = self.finalized_by_height.get_mut(&old_h) {
                        set.remove(hash);
                        if set.is_empty() {
                            self.finalized_by_height.remove(&old_h);
                        }
                    }
                }
            }
            self.unfinalized.remove(hash);
            self.finalized_by_height
                .entry(checkpoint_height)
                .or_default()
                .insert(hash.to_string());
            Ok(())
        } else {
            Err(DagError::NodeNotFound(hash.to_string()))
        }
    }

    pub fn unmark_finalized_batch(&mut self, hashes: &[String]) -> usize {
        let mut count = 0;
        let mut to_restore: Vec<(String, String, Option<u64>)> = Vec::new();
        for hash in hashes {
            if let Some(node) = self.get_node_mut(hash) {
                if node.finalized {
                    let cp_h = node.checkpoint_height;
                    node.finalized = false;
                    node.checkpoint_height = None;
                    let sender = node.tx.tx.from.clone();
                    to_restore.push((hash.clone(), sender, cp_h));
                    count += 1;
                }
            }
        }
        for (hash, sender, cp_h) in to_restore {
            if let Some(h) = cp_h {
                if let Some(set) = self.finalized_by_height.get_mut(&h) {
                    set.remove(&hash);
                    if set.is_empty() {
                        self.finalized_by_height.remove(&h);
                    }
                }
            }
            self.unfinalized.insert(hash.clone());
            self.sender_unfinalized
                .entry(sender)
                .or_default()
                .insert(hash);
        }
        count
    }

    pub fn cleanup_sender_unfinalized_batch(&mut self, hashes: &[String]) {
        for hash in hashes {
            if let Some(node) = self.get_node(hash) {
                let sender = node.tx.tx.from.clone();
                if let Some(set) = self.sender_unfinalized.get_mut(&sender) {
                    set.remove(hash);
                    if set.is_empty() {
                        self.sender_unfinalized.remove(&sender);
                    }
                }
            }
        }
    }

    /// Batch mark multiple transactions as finalized (optimized for checkpoint creation)
    /// Returns the number of transactions successfully marked
    pub fn mark_finalized_batch(&mut self, hashes: &[String], checkpoint_height: u64) -> usize {
        let mut count = 0;
        let mut senders_to_remove: Vec<(String, String)> = Vec::new();
        let mut finalized_hashes: Vec<String> = Vec::new();
        let mut old_heights_to_clean: Vec<(u64, String)> = Vec::new();
        for hash in hashes {
            if let Some(node) = self.get_node_mut(hash) {
                senders_to_remove.push((node.tx.tx.from.clone(), hash.clone()));
                if node.finalized {
                    if let Some(old_h) = node.checkpoint_height {
                        if old_h != checkpoint_height {
                            old_heights_to_clean.push((old_h, hash.clone()));
                        }
                    }
                }
                node.finalized = true;
                node.checkpoint_height = Some(checkpoint_height);
                finalized_hashes.push(hash.clone());
                count += 1;
            }
        }
        for (old_h, hash) in old_heights_to_clean {
            if let Some(set) = self.finalized_by_height.get_mut(&old_h) {
                set.remove(&hash);
                if set.is_empty() {
                    self.finalized_by_height.remove(&old_h);
                }
            }
        }
        let height_set = self
            .finalized_by_height
            .entry(checkpoint_height)
            .or_default();
        for hash in hashes {
            self.unfinalized.remove(hash);
        }
        for hash in finalized_hashes {
            height_set.insert(hash);
        }
        for (sender, hash) in senders_to_remove {
            if let Some(set) = self.sender_unfinalized.get_mut(&sender) {
                set.remove(&hash);
                if set.is_empty() {
                    self.sender_unfinalized.remove(&sender);
                }
            }
        }
        count
    }

    /// Unfinalize a transaction (used during checkpoint chain recovery)
    pub fn unfinalize(&mut self, hash: &str) {
        if let Some(node) = self.get_node_mut(hash) {
            let sender = node.tx.tx.from.clone();
            let cp_h = node.checkpoint_height;
            node.finalized = false;
            node.checkpoint_height = None;
            if let Some(h) = cp_h {
                if let Some(set) = self.finalized_by_height.get_mut(&h) {
                    set.remove(hash);
                    if set.is_empty() {
                        self.finalized_by_height.remove(&h);
                    }
                }
            }
            self.unfinalized.insert(hash.to_string());
            self.sender_unfinalized
                .entry(sender)
                .or_default()
                .insert(hash.to_string());
        }
    }

    pub fn prune_finalized_before(&mut self, min_checkpoint_height: u64) -> usize {
        let unfinalized_ancestors: HashSet<String> = {
            let mut ancestors = HashSet::new();
            for hash in &self.unfinalized {
                if let Some(idx) = self.hash_to_index.get(hash) {
                    if let Some(node) = self.graph.node_weight(*idx) {
                        for parent in &node.tx.tx.parents {
                            ancestors.insert(parent.clone());
                        }
                    }
                }
            }
            ancestors
        };

        let heights_to_prune: Vec<u64> = self
            .finalized_by_height
            .range(..min_checkpoint_height)
            .map(|(&h, _)| h)
            .collect();

        let mut hashes_to_remove: Vec<String> = Vec::new();
        for h in &heights_to_prune {
            if let Some(set) = self.finalized_by_height.get(h) {
                for hash in set {
                    if !self.tips.contains(hash) && !unfinalized_ancestors.contains(hash) {
                        hashes_to_remove.push(hash.clone());
                    }
                }
            }
        }

        let count = hashes_to_remove.len();
        for hash in &hashes_to_remove {
            if let Some(idx) = self.hash_to_index.remove(hash) {
                self.tips.remove(hash);

                if let Some(node) = self.graph.node_weight(idx) {
                    let sender = node.tx.tx.from.clone();
                    if let Some(set) = self.sender_unfinalized.get_mut(&sender) {
                        set.remove(hash);
                        if set.is_empty() {
                            self.sender_unfinalized.remove(&sender);
                        }
                    }
                }

                let parent_hashes: Vec<String> = self
                    .graph
                    .node_weight(idx)
                    .map(|n| n.parents.clone())
                    .unwrap_or_default();

                for parent_hash in &parent_hashes {
                    if let Some(&pidx) = self.hash_to_index.get(parent_hash) {
                        if let Some(parent_node) = self.graph.node_weight_mut(pidx) {
                            parent_node.children.retain(|c| c != hash);
                            if parent_node.children.is_empty() {
                                self.tips.insert(parent_hash.clone());
                            }
                        }
                    }
                }

                let last_idx = NodeIndex::new(self.graph.node_count() - 1);
                let swapped_hash = if idx != last_idx {
                    self.graph.node_weight(last_idx).map(|n| n.hash.clone())
                } else {
                    None
                };

                self.graph.remove_node(idx);

                if let Some(sh) = swapped_hash {
                    self.hash_to_index.insert(sh, idx);
                }
            }
        }

        for h in &heights_to_prune {
            if let Some(set) = self.finalized_by_height.get_mut(h) {
                for hash in &hashes_to_remove {
                    set.remove(hash);
                }
                if set.is_empty() {
                    self.finalized_by_height.remove(h);
                }
            }
        }

        count
    }

    fn prune_oldest(&mut self) -> Result<(), DagError> {
        let target_size = self.max_nodes * 3 / 4;
        let current_size = self.graph.node_count();

        // CRITICAL FIX: If we're severely over limit (2x max_nodes), also prune unfinalized txs
        // This prevents OOM from accumulated unfinalized transactions during sync issues
        let severe_overload = current_size > self.max_nodes * 2;

        // Get current timestamp for age-based pruning of unfinalized txs
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Safety threshold: only prune unfinalized txs older than 10 minutes
        // This ensures they've had ample time to be finalized and makes pruning deterministic
        const UNFINALIZED_PRUNE_AGE_MS: u64 = 10 * 60 * 1000; // 10 minutes

        // First, try to prune finalized non-tip transactions (preferred)
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

        let to_remove_count = current_size.saturating_sub(target_size);
        let mut to_remove: Vec<NodeIndex> = finalized_with_time
            .into_iter()
            .take(to_remove_count)
            .map(|(idx, _)| idx)
            .collect();

        // If we're severely overloaded and couldn't prune enough finalized txs,
        // start pruning OLD unfinalized transactions too (except current tips)
        // Age threshold ensures deterministic pruning across nodes
        if severe_overload && to_remove.len() < to_remove_count {
            let remaining_to_remove = to_remove_count - to_remove.len();

            // Collect OLD unfinalized transactions sorted by timestamp (oldest first)
            let mut unfinalized_with_time: Vec<(NodeIndex, u64)> = self
                .graph
                .node_indices()
                .filter_map(|idx| {
                    self.graph.node_weight(idx).and_then(|node| {
                        // Only prune unfinalized txs that are old enough (deterministic threshold)
                        let tx_age_ms = now_ms.saturating_sub(node.tx.tx.timestamp);
                        if !node.finalized
                            && !self.tips.contains(&node.hash)
                            && tx_age_ms > UNFINALIZED_PRUNE_AGE_MS
                        {
                            Some((idx, node.tx.tx.timestamp))
                        } else {
                            None
                        }
                    })
                })
                .collect();

            unfinalized_with_time.sort_by_key(|(_, ts)| *ts);

            let additional: Vec<NodeIndex> = unfinalized_with_time
                .into_iter()
                .take(remaining_to_remove)
                .map(|(idx, _)| idx)
                .collect();

            to_remove.extend(additional);
        }

        for idx in to_remove.into_iter().rev() {
            if let Some(node) = self.graph.node_weight(idx) {
                let hash = node.hash.clone();
                let sender = node.tx.tx.from.clone();
                self.hash_to_index.remove(&hash);
                self.tips.remove(&hash);
                self.unfinalized.remove(&hash);
                if let Some(set) = self.sender_unfinalized.get_mut(&sender) {
                    set.remove(&hash);
                    if set.is_empty() {
                        self.sender_unfinalized.remove(&sender);
                    }
                }
            }
            self.graph.remove_node(idx);
        }

        self.rebuild_index();

        Ok(())
    }

    fn rebuild_index(&mut self) {
        self.hash_to_index.clear();
        self.unfinalized.clear();
        self.sender_unfinalized.clear();
        self.finalized_by_height.clear();
        for idx in self.graph.node_indices() {
            if let Some(node) = self.graph.node_weight(idx) {
                self.hash_to_index.insert(node.hash.clone(), idx);
                if !node.finalized {
                    self.unfinalized.insert(node.hash.clone());
                    self.sender_unfinalized
                        .entry(node.tx.tx.from.clone())
                        .or_default()
                        .insert(node.hash.clone());
                } else if let Some(h) = node.checkpoint_height {
                    self.finalized_by_height
                        .entry(h)
                        .or_default()
                        .insert(node.hash.clone());
                }
            }
        }
    }

    pub fn remove_nodes_batch(&mut self, hashes: &[String]) -> usize {
        if hashes.is_empty() {
            return 0;
        }

        let evict_set: std::collections::HashSet<&str> =
            hashes.iter().map(|s| s.as_str()).collect();

        let mut seen_indices = std::collections::HashSet::new();
        let mut indices_to_remove: Vec<petgraph::graph::NodeIndex> = hashes
            .iter()
            .filter_map(|hash| self.hash_to_index.get(hash).copied())
            .filter(|idx| seen_indices.insert(*idx))
            .collect();
        indices_to_remove.sort_by_key(|b| std::cmp::Reverse(b.index()));

        let count = indices_to_remove.len();
        if count == 0 {
            return 0;
        }

        for idx in indices_to_remove {
            self.graph.remove_node(idx);
        }

        self.hash_to_index.clear();
        self.unfinalized.clear();
        self.sender_unfinalized.clear();
        self.finalized_by_height.clear();
        self.tips.clear();

        for idx in self.graph.node_indices() {
            if let Some(node) = self.graph.node_weight_mut(idx) {
                node.children.retain(|c| !evict_set.contains(c.as_str()));
                node.parents.retain(|p| !evict_set.contains(p.as_str()));
                node.tx
                    .tx
                    .parents
                    .retain(|p| !evict_set.contains(p.as_str()));
            }
        }

        for idx in self.graph.node_indices() {
            if let Some(node) = self.graph.node_weight(idx) {
                self.hash_to_index.insert(node.hash.clone(), idx);
                if !node.finalized {
                    self.unfinalized.insert(node.hash.clone());
                    self.sender_unfinalized
                        .entry(node.tx.tx.from.clone())
                        .or_default()
                        .insert(node.hash.clone());
                } else if let Some(h) = node.checkpoint_height {
                    self.finalized_by_height
                        .entry(h)
                        .or_default()
                        .insert(node.hash.clone());
                }
                if node.children.is_empty() {
                    self.tips.insert(node.hash.clone());
                }
            }
        }

        count
    }

    pub fn evict_finalized_before(&mut self, boundary_height: u64) -> usize {
        let to_evict: Vec<String> = self
            .graph
            .node_weights()
            .filter(|node| {
                node.finalized
                    && node.checkpoint_height.is_some_and(|h| h < boundary_height)
                    && node.children.iter().all(|child_hash| {
                        self.hash_to_index
                            .get(child_hash)
                            .and_then(|&idx| self.graph.node_weight(idx))
                            .is_none_or(|child| child.finalized)
                    })
            })
            .map(|node| node.hash.clone())
            .collect();

        if to_evict.is_empty() {
            return 0;
        }

        let count = to_evict.len();

        let evict_set: std::collections::HashSet<&str> =
            to_evict.iter().map(|s| s.as_str()).collect();

        let mut indices_to_remove: Vec<petgraph::graph::NodeIndex> = to_evict
            .iter()
            .filter_map(|hash| self.hash_to_index.get(hash).copied())
            .collect();
        indices_to_remove.sort_by_key(|b| std::cmp::Reverse(b.index()));

        for idx in indices_to_remove {
            self.graph.remove_node(idx);
        }

        self.hash_to_index.clear();
        self.unfinalized.clear();
        self.sender_unfinalized.clear();
        self.finalized_by_height.clear();
        self.tips.clear();

        for idx in self.graph.node_indices() {
            if let Some(node) = self.graph.node_weight_mut(idx) {
                node.children.retain(|c| !evict_set.contains(c.as_str()));
                node.parents.retain(|p| !evict_set.contains(p.as_str()));
            }
        }

        for idx in self.graph.node_indices() {
            if let Some(node) = self.graph.node_weight(idx) {
                self.hash_to_index.insert(node.hash.clone(), idx);
                if !node.finalized {
                    self.unfinalized.insert(node.hash.clone());
                    self.sender_unfinalized
                        .entry(node.tx.tx.from.clone())
                        .or_default()
                        .insert(node.hash.clone());
                } else if let Some(h) = node.checkpoint_height {
                    self.finalized_by_height
                        .entry(h)
                        .or_default()
                        .insert(node.hash.clone());
                }
                if node.children.is_empty() {
                    self.tips.insert(node.hash.clone());
                }
            }
        }

        count
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
        if let Some(node) = self.graph.node_weight(idx) {
            let sender = node.tx.tx.from.clone();
            let cp_h = node.checkpoint_height;
            if let Some(set) = self.sender_unfinalized.get_mut(&sender) {
                set.remove(hash);
                if set.is_empty() {
                    self.sender_unfinalized.remove(&sender);
                }
            }
            if let Some(h) = cp_h {
                if let Some(set) = self.finalized_by_height.get_mut(&h) {
                    set.remove(hash);
                    if set.is_empty() {
                        self.finalized_by_height.remove(&h);
                    }
                }
            }
        }

        let parent_hashes: Vec<String> = self
            .graph
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
        let ancestors_a: HashSet<String> =
            self.get_ancestors(hash_a, usize::MAX).into_iter().collect();
        let ancestors_b = self.get_ancestors(hash_b, usize::MAX);

        ancestors_b
            .into_iter()
            .find(|ancestor| ancestors_a.contains(ancestor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SignedTransaction, Transaction};

    fn make_tx(hash: &str, parents: Vec<String>) -> DagNode {
        let tx = Transaction {
            from: "sender".to_string(),
            to: "receiver".to_string(),
            amount: 1,
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
            partition_epoch: None,
            rolled_back: false,
            fast_path_cert: None,
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
        dag.add_node(make_tx("a", vec!["root".to_string()]))
            .unwrap();
        dag.add_node(make_tx("b", vec!["root".to_string()]))
            .unwrap();
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
        dag.add_node(make_tx("a", vec!["root".to_string()]))
            .unwrap();
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
        dag.add_node(make_tx("a", vec!["root".to_string()]))
            .unwrap();
        dag.add_node(make_tx("b", vec!["root".to_string()]))
            .unwrap();
        dag.add_node(make_tx("a1", vec!["a".to_string()])).unwrap();
        dag.add_node(make_tx("b1", vec!["b".to_string()])).unwrap();

        let common = dag.find_common_ancestor("a1", "b1");
        assert_eq!(common, Some("root".to_string()));
    }

    #[test]
    fn test_missing_parents_accepted() {
        let mut dag = Dag::new(100);

        dag.add_node(make_tx("root", vec![])).unwrap();

        let result = dag.add_node(make_tx(
            "child",
            vec!["root".to_string(), "missing_parent".to_string()],
        ));
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

        let (linked, total) = dag
            .add_node_with_stats(make_tx(
                "child",
                vec![
                    "root".to_string(),
                    "missing1".to_string(),
                    "missing2".to_string(),
                ],
            ))
            .unwrap();

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
