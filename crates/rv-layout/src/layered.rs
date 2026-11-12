//! Layered (Sugiyama-style) layout of a directed graph, left to right:
//! cycles are broken, nodes get ranks by longest path, nodes within a rank
//! are ordered by the barycenter of their neighbors to reduce crossings.

use emath::{Pos2, Vec2, pos2, vec2};

#[derive(Clone, Debug)]
pub struct LayeredLayout {
    /// Top-left corner of every node.
    pub positions: Vec<Pos2>,
    /// Column of every node.
    pub ranks: Vec<usize>,
    pub size: Vec2,
}

/// `sizes`: size of each node; `edges`: (from, to); `gap`: space between
/// columns (x) and between nodes of a column (y).
pub fn layered(sizes: &[Vec2], edges: &[(usize, usize)], gap: Vec2) -> LayeredLayout {
    let dag = acyclic(&successors(sizes.len(), edges));
    let ranks = longest_path_ranks(&dag);
    let layers = ordered_layers(&dag, &ranks);
    let (positions, size) = coordinates(&layers, sizes, gap);
    LayeredLayout {
        positions,
        ranks,
        size,
    }
}

/// Successor lists of `n` nodes without self loops, duplicates and edges
/// to unknown nodes.
fn successors(n: usize, edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in edges {
        if a != b && a < n && b < n && !out[a].contains(&b) {
            out[a].push(b);
        }
    }
    out
}

/// The nodes of every rank, ordered to reduce crossings: sweeps alternately
/// down and up the ranks, sorting each by the barycenter of its neighbors
/// in the rank before.
fn ordered_layers(dag: &[Vec<usize>], ranks: &[usize]) -> Vec<Vec<usize>> {
    let n = dag.len();
    let rank_count = ranks.iter().copied().max().map_or(0, |r| r + 1);

    // Initial order: by index within each rank.
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); rank_count];
    for v in 0..n {
        layers[ranks[v]].push(v);
    }
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (a, succ) in dag.iter().enumerate() {
        for &b in succ {
            preds[b].push(a);
        }
    }
    let mut pos = vec![0.0f32; n];
    for layer in &layers {
        number(layer, &mut pos);
    }
    for sweep in 0..8 {
        let down = sweep % 2 == 0;
        let (order, neighbors): (Vec<usize>, &[Vec<usize>]) = if down {
            ((1..rank_count).collect(), &preds)
        } else {
            ((0..rank_count.saturating_sub(1)).rev().collect(), dag)
        };
        for r in order {
            sort_by_barycenter(&mut layers[r], neighbors, &pos);
            number(&layers[r], &mut pos);
        }
    }
    layers
}

/// Sets the position of every node of `layer` to its index in the layer.
fn number(layer: &[usize], pos: &mut [f32]) {
    for (i, &v) in layer.iter().enumerate() {
        pos[v] = i as f32;
    }
}

/// Sorts `layer` by the mean position of each node's neighbors; nodes
/// without neighbors keep their position.
fn sort_by_barycenter(layer: &mut Vec<usize>, neighbors: &[Vec<usize>], pos: &[f32]) {
    let mut keyed: Vec<(f32, usize)> = layer
        .iter()
        .map(|&v| {
            let ns = &neighbors[v];
            let key = if ns.is_empty() {
                pos[v]
            } else {
                ns.iter().map(|&u| pos[u]).sum::<f32>() / ns.len() as f32
            };
            (key, v)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    *layer = keyed.into_iter().map(|(_, v)| v).collect();
}

/// Top-left corner of every node and the total size: columns side by side,
/// each column centered vertically.
fn coordinates(layers: &[Vec<usize>], sizes: &[Vec2], gap: Vec2) -> (Vec<Pos2>, Vec2) {
    let col_width: Vec<f32> = layers
        .iter()
        .map(|l| l.iter().map(|&v| sizes[v].x).fold(0.0, f32::max))
        .collect();
    let col_height: Vec<f32> = layers
        .iter()
        .map(|l| {
            l.iter().map(|&v| sizes[v].y).sum::<f32>() + gap.y * l.len().saturating_sub(1) as f32
        })
        .collect();
    let height = col_height.iter().copied().fold(0.0, f32::max);
    let mut positions = vec![Pos2::ZERO; sizes.len()];
    let mut x = 0.0;
    for (r, layer) in layers.iter().enumerate() {
        let mut y = (height - col_height[r]) / 2.0;
        for &v in layer {
            positions[v] = pos2(x, y);
            y += sizes[v].y + gap.y;
        }
        x += col_width[r] + gap.x;
    }
    let width = (x - gap.x).max(0.0);
    (positions, vec2(width, height))
}

/// Successor lists without the edges that close a cycle (found by DFS,
/// starting from nodes without predecessors).
fn acyclic(out: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = out.len();
    let indegree = indegrees(out);
    let mut starts: Vec<usize> = (0..n).collect();
    starts.sort_by_key(|&v| (indegree[v] != 0, v));
    // 0 = new, 1 = on stack, 2 = done
    let mut state = vec![0u8; n];
    let mut dag: Vec<Vec<usize>> = vec![Vec::new(); n];
    for s in starts {
        if state[s] != 0 {
            continue;
        }
        let mut stack = vec![(s, 0usize)];
        state[s] = 1;
        while let Some(&mut (v, ref mut i)) = stack.last_mut() {
            if *i < out[v].len() {
                let w = out[v][*i];
                *i += 1;
                match state[w] {
                    0 => {
                        dag[v].push(w);
                        state[w] = 1;
                        stack.push((w, 0));
                    }
                    1 => {} // back edge: dropped
                    _ => dag[v].push(w),
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
    dag
}

/// Number of predecessors of every node.
fn indegrees(successors: &[Vec<usize>]) -> Vec<usize> {
    let mut indegree = vec![0; successors.len()];
    for succ in successors {
        for &b in succ {
            indegree[b] += 1;
        }
    }
    indegree
}

/// Rank of every node: the length of the longest path leading to it.
fn longest_path_ranks(dag: &[Vec<usize>]) -> Vec<usize> {
    let n = dag.len();
    let mut indegree = indegrees(dag);
    let mut queue: Vec<usize> = (0..n).filter(|&v| indegree[v] == 0).collect();
    let mut rank = vec![0; n];
    let mut head = 0;
    while head < queue.len() {
        let v = queue[head];
        head += 1;
        for &w in &dag[v] {
            rank[w] = rank[w].max(rank[v] + 1);
            indegree[w] -= 1;
            if indegree[w] == 0 {
                queue.push(w);
            }
        }
    }
    rank
}

#[cfg(test)]
mod tests {
    use super::*;
    use emath::Rect;

    fn rects(l: &LayeredLayout, sizes: &[Vec2]) -> Vec<Rect> {
        l.positions
            .iter()
            .zip(sizes)
            .map(|(p, s)| Rect::from_min_size(*p, *s))
            .collect()
    }

    #[test]
    fn chain_goes_left_to_right() {
        let sizes = vec![vec2(50.0, 20.0); 3];
        let l = layered(&sizes, &[(0, 1), (1, 2)], vec2(30.0, 10.0));
        assert_eq!(l.ranks, [0, 1, 2]);
        assert!(l.positions[0].x < l.positions[1].x && l.positions[1].x < l.positions[2].x);
    }

    #[test]
    fn cycles_and_shortcuts_are_handled() {
        // 0 -> 1 -> 2 -> 0 (cycle), 0 -> 2 (shortcut), 3 isolated.
        let sizes = vec![vec2(40.0, 20.0); 4];
        let l = layered(&sizes, &[(0, 1), (1, 2), (2, 0), (0, 2)], vec2(30.0, 10.0));
        assert_eq!(&l.ranks[..3], [0, 1, 2], "longest path, back edge ignored");
        // No overlaps.
        let r = rects(&l, &sizes);
        for i in 0..r.len() {
            for j in i + 1..r.len() {
                assert!(!r[i].intersects(r[j]), "{i} and {j} overlap");
            }
        }
    }

    #[test]
    fn barycenters_reduce_crossings() {
        // Two sources, each with its own target; listed crosswise.
        // 0 -> 3, 1 -> 2: the targets should be reordered to 3 above 2.
        let sizes = vec![vec2(40.0, 20.0); 4];
        let l = layered(&sizes, &[(0, 3), (1, 2)], vec2(30.0, 10.0));
        assert!(l.positions[3].y < l.positions[2].y);
    }
}
