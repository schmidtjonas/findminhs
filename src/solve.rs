use crate::activity::Activities;
use crate::instance::{Instance, NodeIdx};
use crate::small_indices::SmallIdx;
use crate::subsuperset;
use crate::subsuperset::Reduction;
use anyhow::Result;
use log::{debug, info, trace, warn};
use rand::{Rng, SeedableRng};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub iterations: usize,
    pub subsuper_prune_time: Duration,
}

#[derive(Debug, Clone)]
struct State<R: Rng> {
    rng: R,
    incomplete_hs: Vec<NodeIdx>,
    discarded: Vec<NodeIdx>,
    best_known: Vec<NodeIdx>,
    activities: Activities,
    stats: Stats,
}

#[derive(Debug, Clone)]
#[allow(clippy::module_name_repetitions)]
pub struct SolveResult {
    pub hs_size: usize,
    pub greedy_size: usize,
    pub solve_time: f64,
    pub stats: Stats,
}

fn greedy_approx(instance: &mut Instance) -> Vec<NodeIdx> {
    let time_start = Instant::now();
    let mut hs = vec![];
    while !instance.edges().is_empty() {
        let mut max_degree = (0, NodeIdx::INVALID);
        for &node in instance.nodes() {
            max_degree = max_degree.max((instance.node_degree(node), node));
        }
        instance.delete_node(max_degree.1);
        instance.delete_incident_edges(max_degree.1);
        hs.push(max_degree.1);
    }
    for &node in hs.iter().rev() {
        instance.restore_incident_edges(node);
        instance.restore_node(node);
    }
    info!(
        "Greedy hs of size {} ({:.2?})",
        hs.len(),
        Instant::now() - time_start
    );
    hs
}

fn can_prune(instance: &Instance, state: &State<impl Rng>) -> bool {
    let max_node_degree = instance.max_node_degree();
    let num_edges = instance.num_edges();
    debug_assert!(max_node_degree > 0);
    let rem_lower_bound = (num_edges + max_node_degree - 1) / max_node_degree;
    let lower_bound = state.incomplete_hs.len() + rem_lower_bound;
    lower_bound >= state.best_known.len()
}

fn branch_on(node: NodeIdx, instance: &mut Instance, state: &mut State<impl Rng>) {
    trace!("Branching on {}", node);

    // Randomize branching order
    if state.rng.gen() {
        instance.delete_node(node);
        state.activities.delete(node);
        state.discarded.push(node);
        solve_recursive(instance, state);
        state.discarded.pop();
        instance.delete_incident_edges(node);
        state.incomplete_hs.push(node);
        solve_recursive(instance, state);
        state.incomplete_hs.pop();
        instance.restore_incident_edges(node);
        instance.restore_node(node);
        state.activities.restore(node);
    } else {
        instance.delete_node(node);
        state.activities.delete(node);
        instance.delete_incident_edges(node);
        state.incomplete_hs.push(node);
        solve_recursive(instance, state);
        state.incomplete_hs.pop();
        instance.restore_incident_edges(node);
        state.discarded.push(node);
        solve_recursive(instance, state);
        state.discarded.pop();
        instance.restore_node(node);
        state.activities.restore(node);
    }
}

fn solve_recursive(instance: &mut Instance, state: &mut State<impl Rng>) {
    if instance.edges().is_empty() {
        if state.incomplete_hs.len() < state.best_known.len() {
            info!("Found HS of size {}", state.incomplete_hs.len());
            state.best_known.truncate(state.incomplete_hs.len());
            state.best_known.copy_from_slice(&state.incomplete_hs);
        } else {
            warn!(
                "Found HS larger than best known ({} vs. {}), should have been pruned",
                state.incomplete_hs.len(),
                state.best_known.len()
            );
        }
        return;
    }

    // Don't count the last iteration where we find a new best HS, since they
    // are comparatively very cheap
    state.stats.iterations += 1;

    // Don't prune on the first iteration, we already do it before calculating
    // the greedy approximation
    let reduction = if state.stats.iterations > 1 {
        subsuperset::prune(instance, &mut state.stats)
    } else {
        Reduction::default()
    };
    for node in reduction.nodes() {
        state.activities.delete(node);
    }

    if can_prune(instance, state) {
        #[allow(clippy::cast_precision_loss)]
        let bump = if cfg!(feature = "relative-activity") {
            let depth = state.incomplete_hs.len() + state.discarded.len();
            1.0 / depth as f64
        } else {
            1.0
        };

        if !cfg!(feature = "disable-activity") {
            for &node in &state.incomplete_hs {
                state.activities.bump(node, (bump, 0.0));
            }

            for &node in &state.discarded {
                state.activities.bump(node, (0.0, bump));
            }

            state.activities.decay();
        }
    } else if let Some((_edge, node)) = instance.degree_1_edge() {
        instance.delete_node(node);
        instance.delete_incident_edges(node);
        state.activities.delete(node);
        state.incomplete_hs.push(node);
        solve_recursive(instance, state);
        state.incomplete_hs.pop();
        state.activities.restore(node);
        instance.restore_incident_edges(node);
        instance.restore_node(node);
    } else {
        let node = if cfg!(feature = "disable-activity") {
            use rand::seq::SliceRandom;
            *instance
                .nodes()
                .choose(&mut state.rng)
                .expect("Check for no nodes failed")
        } else {
            state.activities.highest()
        };
        branch_on(node, instance, state);
    }

    reduction.restore(instance);
    for node in reduction.nodes() {
        state.activities.restore(node);
    }
}

pub fn solve(instance: &mut Instance, rng: impl Rng + SeedableRng) -> Result<SolveResult> {
    let time_start = Instant::now();
    let mut stats = Stats::default();
    subsuperset::prune(instance, &mut stats);
    info!("Initial reduction time: {:.2?}", stats.subsuper_prune_time);
    let approx = greedy_approx(instance);
    let greedy_size = approx.len();
    let activities = Activities::new(instance);
    let mut state = State {
        rng,
        incomplete_hs: vec![],
        discarded: vec![],
        best_known: approx,
        activities,
        stats,
    };
    solve_recursive(instance, &mut state);
    let solve_time = Instant::now() - time_start;
    info!(
        "Solving took {} iterations ({:.2?})",
        state.stats.iterations, solve_time
    );
    debug!(
        "Final HS (size {}): {:?}",
        state.best_known.len(),
        &state.best_known
    );
    Ok(SolveResult {
        hs_size: state.best_known.len(),
        greedy_size,
        solve_time: solve_time.as_secs_f64(),
        stats: state.stats,
    })
}
