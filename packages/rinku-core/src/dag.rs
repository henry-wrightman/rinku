use crate::types::DagNode;
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::{HashMap, HashSet, VecDeque};
use thiserror::Error;

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
    max_nodes: usize,
}

impl Dag {
    pub fn new(max_nodes: usize) -> Self {
        Self {
            graph: DiGraph::new(),
            hash_to_index: HashMap::new(),
            tips: HashSet::new(),
            max_nodes,
        }
    }

    pub fn add_node(&mut self, node: DagNode) -> Result<(), DagError> {
        if self.hash_to_index.contains_key(&node.hash) {
            return Ok(());
        }

        for parent_hash in &node.parents {
            if !parent_hash.is_empty() && !self.hash_to_index.contains_key(parent_hash) {
                return Err(DagError::InvalidParent(parent_hash.clone()));
            }
        }

        let hash = node.hash.clone();
        let parents = node.parents.clone();

        let node_idx = self.graph.add_node(node);
        self.hash_to_index.insert(hash.clone(), node_idx);

        for parent_hash in &parents {
            if !parent_hash.is_empty() {
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

    pub fn all_transactions(&self) -> Vec<crate::types::SignedTransaction> {
        self.graph.node_weights().map(|n| n.tx.clone()).collect()
    }

    pub fn get_unfinalized_nodes(&self) -> Vec<&DagNode> {
        self.graph
            .node_weights()
            .filter(|n| !n.finalized)
            .collect()
    }

    pub fn mark_finalized(&mut self, hash: &str, checkpoint_height: u64) -> Result<(), DagError> {
        if let Some(node) = self.get_node_mut(hash) {
            node.finalized = true;
            node.checkpoint_height = Some(checkpoint_height);
            Ok(())
        } else {
            Err(DagError::NodeNotFound(hash.to_string()))
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
            }
            self.graph.remove_node(idx);
        }

        self.rebuild_index();

        Ok(())
    }

    fn rebuild_index(&mut self) {
        self.hash_to_index.clear();
        for idx in self.graph.node_indices() {
            if let Some(node) = self.graph.node_weight(idx) {
                self.hash_to_index.insert(node.hash.clone(), idx);
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
}
