use crate::{
    create_idx_struct,
    instance::{EdgeIdx, Instance},
    small_indices::SmallIdx,
};
#[cfg(feature = "local-search")]
use crate::{
    data_structures::subset_trie::SubsetTrie, instance::NodeIdx, small_indices::IdxHashSet,
};
#[cfg(feature = "local-search")]
use std::iter::Peekable;

create_idx_struct!(PackingIdx);

#[cfg(feature = "local-search")]
#[derive(Clone)]
struct SetMinusIterator<T, I1, I2>(Peekable<I1>, Peekable<I2>)
where
    I1: Iterator<Item = T>,
    I2: Iterator<Item = T>;

#[cfg(feature = "local-search")]
impl<T, I1, I2> SetMinusIterator<T, I1, I2>
where
    I1: Iterator<Item = T>,
    I2: Iterator<Item = T>,
{
    fn new(
        set: impl IntoIterator<IntoIter = I1>,
        removed_set: impl IntoIterator<IntoIter = I2>,
    ) -> Self {
        Self(
            set.into_iter().peekable(),
            removed_set.into_iter().peekable(),
        )
    }
}

#[cfg(feature = "local-search")]
impl<T, I1, I2> Iterator for SetMinusIterator<T, I1, I2>
where
    I1: Iterator<Item = T>,
    I2: Iterator<Item = T>,
    T: Ord,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.0.peek(), self.1.peek()) {
                (None, _) => return None,
                (Some(_), None) => return self.0.next(),
                (Some(item1), Some(item2)) if *item1 < *item2 => return self.0.next(),
                (Some(item1), Some(item2)) if *item1 == *item2 => {
                    self.0.next();
                    self.1.next();
                    continue;
                }
                (Some(_), Some(_)) => {
                    self.1.next();
                    continue;
                }
            }
        }
    }
}

#[cfg(feature = "local-search")]
fn find_two_opt_swap(
    instance: &Instance,
    available_nodes: &mut Vec<NodeIdx>,
    packing: &[EdgeIdx],
    blocked_by: &[Vec<EdgeIdx>],
    hit_by: &[PackingIdx],
) -> Option<(PackingIdx, (EdgeIdx, EdgeIdx))> {
    available_nodes.clear();
    available_nodes.extend(
        instance
            .nodes()
            .iter()
            .copied()
            .filter(|node_idx| !hit_by[node_idx.idx()].valid()),
    );

    for (blocking_idx, blocked) in blocked_by.iter().enumerate() {
        if blocked.is_empty() {
            continue;
        }

        let blocking_edge = packing[blocking_idx];
        available_nodes.extend(instance.edge(blocking_edge));
        available_nodes.sort_unstable();
        let mut trie: SubsetTrie<_, EdgeIdx, _> = SubsetTrie::new(instance.num_nodes_total());

        for &blocked_edge in blocked {
            let available_iter =
                SetMinusIterator::new(available_nodes.iter().copied(), instance.edge(blocked_edge));
            let other_edge = trie.find_subset(available_iter);

            if other_edge.valid() {
                return Some((PackingIdx::from(blocking_idx), (blocked_edge, other_edge)));
            }

            trie.insert(blocked_edge, instance.edge(blocked_edge));
        }

        available_nodes.retain(|node_idx| !hit_by[node_idx.idx()].valid());
    }

    None
}

#[cfg(feature = "local-search")]
fn improve_packing_by_local_search(instance: &Instance, mut packing: Vec<EdgeIdx>) -> Vec<EdgeIdx> {
    let packing_set: IdxHashSet<_> = packing.iter().copied().collect();
    let mut remaining: Vec<_> = instance
        .edges()
        .iter()
        .copied()
        .filter(|edge_idx| !packing_set.contains(edge_idx))
        .collect();

    // Reuse some allocations across local search iterations
    let mut hit_by = vec![PackingIdx::INVALID; instance.num_nodes_total()];
    let mut blocked_by: Vec<Vec<_>> = Vec::new();
    let mut available_nodes = Vec::new();

    loop {
        // For each node, calculate which packing edge is hitting it (if any)
        hit_by.fill(PackingIdx::INVALID);
        for (packing_idx, &packing_edge) in packing.iter().enumerate() {
            for node_idx in instance.edge(packing_edge) {
                hit_by[node_idx.idx()] = PackingIdx::from(packing_idx);
            }
        }

        // Group remaining edges only blocked by a single packing edge by the blocking packing edge
        for blocked_by_list in &mut blocked_by {
            blocked_by_list.clear();
        }
        blocked_by.resize(packing.len(), Vec::new());
        for &remaining_edge in &remaining {
            let mut blocking_idx = PackingIdx::INVALID;
            for node_idx in instance.edge(remaining_edge) {
                if !hit_by[node_idx.idx()].valid() {
                    continue;
                }

                if blocking_idx.valid() && blocking_idx != hit_by[node_idx.idx()] {
                    // Found second edge blocking this ones inclusion
                    blocking_idx = PackingIdx::INVALID;
                    break;
                }
                blocking_idx = hit_by[node_idx.idx()];
            }

            // We assume that each remaining edge is blocked by at least one edge, thus could not
            // simply be added to the packing. Thus, blocking_idx is invalid if and only if this
            // edge was blocked by multiple packing edges.
            if blocking_idx.valid() {
                blocked_by[blocking_idx.idx()].push(remaining_edge);
            }
        }

        let two_opt_swap = find_two_opt_swap(
            instance,
            &mut available_nodes,
            &packing,
            &blocked_by,
            &hit_by,
        );
        let (removed_edge_packing_idx, (added_edge1, added_edge2)) = match two_opt_swap {
            Some(tuple) => tuple,
            None => return packing,
        };

        let removed_edge = packing[removed_edge_packing_idx.idx()];
        packing.retain(|&edge_idx| edge_idx != removed_edge);
        remaining.retain(|&edge_idx| edge_idx != added_edge1 && edge_idx != added_edge2);
        packing.push(added_edge1);
        packing.push(added_edge2);
        remaining.push(removed_edge);

        // Due to the swap, other edges previously blocked by removed_edge might now be addable to
        // the packing. Since we generally assume that no edge can just be added to the packing, we
        // find and add them here.
        for node_idx in instance.edge(removed_edge) {
            hit_by[node_idx.idx()] = PackingIdx::INVALID;
        }

        // Dummy packing idx used to mark hit nodes (since we only care whether nodes are hit here,
        // not by whom)
        let dummy_packing_idx = PackingIdx(0);
        for node_idx in instance.edge(added_edge1).chain(instance.edge(added_edge2)) {
            hit_by[node_idx.idx()] = dummy_packing_idx;
        }

        for &packing_candidate_edge in &blocked_by[removed_edge_packing_idx.idx()] {
            if instance
                .edge(packing_candidate_edge)
                .all(|node_idx| !hit_by[node_idx.idx()].valid())
            {
                packing.push(packing_candidate_edge);
                remaining.retain(|&edge_idx| edge_idx != packing_candidate_edge);
                for node_idx in instance.edge(packing_candidate_edge) {
                    hit_by[node_idx.idx()] = dummy_packing_idx;
                }
            }
        }
    }
}

pub fn pack_edges_without_local_search(
    instance: &Instance,
    edge_sort_keys: &[(u32, u32)],
) -> Vec<EdgeIdx> {
    let mut packing: Vec<_> = instance.edges().to_vec();
    packing.sort_unstable_by_key(|&edge_idx| edge_sort_keys[edge_idx.idx()]);

    let mut free = vec![true; instance.num_edges_total()];
    packing.retain(|&edge_idx| {
        if !free[edge_idx.idx()] {
            return false;
        }

        for node_idx in instance.edge(edge_idx) {
            for node_edge_idx in instance.node(node_idx) {
                free[node_edge_idx.idx()] = false;
            }
        }

        true
    });

    packing
}

pub fn pack_edges(instance: &Instance) -> (Vec<EdgeIdx>, Vec<(u32, u32)>) {
    let mut edge_sort_keys = vec![(0, 0); instance.num_edges_total()];
    for &edge_idx in instance.edges() {
        edge_sort_keys[edge_idx.idx()] =
            instance
                .edge(edge_idx)
                .fold((0, 0), |(sum, max), node_idx| {
                    let degree = instance.node_degree(node_idx) as u32;
                    (sum + degree, max.max(degree))
                })
    }
    let packing = pack_edges_without_local_search(instance, &edge_sort_keys);

    #[cfg(feature = "local-search")]
    let packing = improve_packing_by_local_search(instance, packing);

    (packing, edge_sort_keys)
}

pub fn calculate(instance: &Instance, packing: &[EdgeIdx], partial_size: usize) -> usize {
    let mut degree = vec![0; instance.num_nodes_total()];
    let mut covered_edges = 0;
    for &node_idx in instance.nodes() {
        degree[node_idx.idx()] = instance.node_degree(node_idx);
    }

    for &packed_edge in packing {
        let max_degree_node = instance
            .edge(packed_edge)
            .max_by_key(|&node_idx| instance.node_degree(node_idx))
            .expect("Empty edge in packing");
        covered_edges += instance.node_degree(max_degree_node);

        for node_idx in instance.edge(packed_edge) {
            degree[node_idx.idx()] -= 1;
        }

        degree[max_degree_node.idx()] = 0;
    }

    degree.sort_unstable();
    let sum_bound = degree
        .into_iter()
        .rev()
        .take_while(|&degree| {
            if covered_edges < instance.num_edges() {
                covered_edges += degree;
                true
            } else {
                false
            }
        })
        .count();

    partial_size + packing.len() + sum_bound
}
