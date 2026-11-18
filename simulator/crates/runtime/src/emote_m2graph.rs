//! Independent translation of the fixed driver's 9da0 search and 9710 choice.
//! Node groups identify zero-distance jumps; they are not numeric intervals.
//! Search order, persistent visited values, ten passes and equal-cost order
//! are observable. A generic shortest-path algorithm changes those rules.

use super::{EmoteError, PsbValue, Result};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy)]
pub(super) struct Segment {
    pub from: f32,
    pub to: f32,
}

#[derive(Debug, Clone)]
pub(super) struct Route {
    pub segments: VecDeque<Segment>,
    pub length: f32,
}

struct Graph {
    edges: Vec<[f32; 2]>,
    nodes: Vec<Vec<f32>>,
}

fn invalid(reason: &'static str) -> EmoteError {
    EmoteError::InvalidResource(reason)
}

fn finite(value: &PsbValue) -> Result<f32> {
    value
        .number()
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
        .ok_or_else(|| invalid("M2 graph finite endpoint"))
}

impl Graph {
    fn parse(control: &PsbValue) -> Result<Self> {
        let edges = control
            .get("edge")
            .and_then(PsbValue::list)
            .filter(|v| v.len() <= 4096)
            .ok_or_else(|| invalid("M2 graph edge list"))?;
        let nodes = control
            .get("node")
            .and_then(PsbValue::list)
            .filter(|v| v.len() <= 4096)
            .ok_or_else(|| invalid("M2 graph node list"))?;
        let edges = edges
            .iter()
            .map(|edge| {
                let values = edge
                    .list()
                    .filter(|v| v.len() == 2)
                    .ok_or_else(|| invalid("M2 graph edge pair"))?;
                let pair = [finite(&values[0])?, finite(&values[1])?];
                if pair[0] > pair[1] {
                    return Err(invalid("M2 graph reversed edge"));
                }
                Ok(pair)
            })
            .collect::<Result<_>>()?;
        let mut total = 0usize;
        let nodes = nodes
            .iter()
            .map(|node| {
                let values = node.list().ok_or_else(|| invalid("M2 graph node group"))?;
                total = total
                    .checked_add(values.len())
                    .filter(|n| *n <= 65_536)
                    .ok_or(EmoteError::Capacity)?;
                values.iter().map(finite).collect::<Result<_>>()
            })
            .collect::<Result<_>>()?;
        Ok(Self { edges, nodes })
    }

    fn edge(&self, value: f32) -> Option<usize> {
        //9a20: first inclusive edge, including degenerate edges.
        self.edges
            .iter()
            .position(|v| value >= v[0] && value <= v[1])
    }

    fn node(&self, value: f32) -> Option<usize> {
        //9a60: exact float membership; never nearest-value matching.
        self.nodes.iter().position(|v| v.contains(&value))
    }

    fn interior_node(&self, low: f32, high: f32) -> Option<usize> {
        //9b00: first group with any point inside the current edge.
        self.nodes
            .iter()
            .position(|v| v.iter().any(|v| *v >= low && *v <= high))
    }

    fn unvisited(&self, node: usize, visited: &[f32]) -> Option<f32> {
        //9ba0: actual list order, skipping exact values already visited.
        //The original uses -1 as a sentinel even if present in a node.
        self.nodes[node]
            .iter()
            .copied()
            .find(|v| !visited.contains(v))
            .filter(|v| *v != -1.0)
    }
}

fn append(route: &mut Route, from: f32, to: f32) -> Result<()> {
    route.length += (to - from).abs();
    if !route.length.is_finite() {
        return Err(invalid("M2 graph path length overflow"));
    }
    route.segments.push_back(Segment { from, to });
    Ok(())
}

pub(super) fn plan(control: &PsbValue, from: f32, target: f32) -> Result<Route> {
    let graph = Graph::parse(control)?;
    let mut visited = Vec::new();
    let mut candidates: Vec<Route> = Vec::new();
    let mut steps = 0usize;
    //a737: ten completed search passes. The visited vector survives resets.
    for _ in 0..10 {
        if graph.edge(from).is_none() || graph.edge(target).is_none() {
            break;
        }
        let mut current = from;
        let mut route = Route {
            segments: VecDeque::new(),
            length: 0.0,
        };
        let mut previous = Vec::new();
        let mut forward = true;
        let mut state = 1;
        let mut succeeded = false;
        loop {
            steps += 1;
            if steps > 100_000 || visited.len() > 100_000 {
                return Err(EmoteError::Capacity);
            }
            let edge = graph
                .edge(current)
                .ok_or_else(|| invalid("M2 graph node outside every edge"))?;
            let [low, high] = graph.edges[edge];
            if state == 1 {
                if graph.edge(target) == Some(edge) {
                    //a49c: remove both final-edge endpoints from visited,
                    //including all duplicate occurrences, before next pass.
                    visited.retain(|v| *v != low && *v != high);
                    append(&mut route, current, target)?;
                    succeeded = true;
                    break;
                }
                state = if forward { 2 } else { 3 };
            }
            let endpoint = if state == 2 { high } else { low };
            let next = if let Some(node) = graph.node(endpoint) {
                visited.push(endpoint);
                graph.unvisited(node, &visited)
            } else {
                None
            };
            if let Some(next) = next {
                append(&mut route, current, endpoint)?;
                visited.push(next);
                previous.push(current);
                current = next;
                state = 1;
                continue;
            }
            if let Some(prior) = previous.pop() {
                current = prior;
                //9cd0: subtract the removed segment's own length in f32.
                if let Some(segment) = route.segments.pop_back() {
                    route.length -= (segment.to - segment.from).abs();
                }
                //a189/a2c0: retain state2 or state3 when backtracking.
                continue;
            }
            if state == 2 {
                forward = false;
                state = 3;
                continue;
            }
            //a2dd: after exhausting both endpoint directions, inspect the
            //first node group crossing this edge. Its first unused point is
            //the segment end, and its next unused point is a free jump.
            let Some(node) = graph.interior_node(low, high) else {
                break;
            };
            //a33c appends the sentinel before its second query as well.
            let endpoint = graph.unvisited(node, &visited).unwrap_or(-1.0);
            visited.push(endpoint);
            let Some(next) = graph.unvisited(node, &visited) else {
                break;
            };
            append(&mut route, current, endpoint)?;
            visited.push(next);
            previous.push(current);
            current = next;
            state = 1;
        }
        if !succeeded {
            //a71f: failed candidates carry cost -1 and stop the passes.
            //9710 ignores them. Preserve earlier successful candidates.
            break;
        }
        let repeat = !previous.is_empty();
        //a626: discard equal-cost routes even when their segments differ.
        if !candidates.iter().any(|old| old.length == route.length) {
            candidates.push(route);
        }
        if !repeat {
            break;
        }
    }
    //97c0: strict lower cost keeps the first route on ties.
    let mut best: Option<Route> = None;
    for candidate in candidates {
        if best
            .as_ref()
            .is_none_or(|old| candidate.length < old.length)
        {
            best = Some(candidate);
        }
    }
    //99db: the original's no-route result is exactly target->target, length0.
    //This is a recovered driver branch, not a synthetic success fallback.
    Ok(best.unwrap_or_else(|| Route {
        segments: VecDeque::from([Segment {
            from: target,
            to: target,
        }]),
        length: 0.0,
    }))
}
