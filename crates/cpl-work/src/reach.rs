//! Reachability through the `after` edges of one change's tasks, shared by `cpl work check`
//! (`validate`) and `ChangeRun::open` (`scheduler`): the strongly connected components, found
//! iteratively (a chain of thousands of tasks needs no deep stack), and for each component the
//! tasks it reaches as a bitset, built once in reverse topological order. A query is then one
//! bit test, and the memory is `n²/8` bytes for `n` tasks, which `MAX_TASKS_PER_CHANGE` bounds
//! (two megabytes); a set per task, filled by a walk from every task, took time and memory
//! quadratic in `n` with large constants (1.5 GB for 12,000 chained tasks).

/// The most tasks one change may declare (`work_too_many_tasks`): the overlap rule compares
/// every pair of unordered tasks, so a change's cost grows with the square of its tasks.
pub const MAX_TASKS_PER_CHANGE: usize = 4096;

/// The reachability of a graph on `0..n` (see the module docs).
pub struct Reach {
    /// The component of each node.
    comp: Vec<usize>,
    /// The members of each component, in increasing order; components in the order Tarjan's
    /// algorithm closes them (successors first).
    members: Vec<Vec<usize>>,
    /// Whether each component has a cycle (more than one member, or a self-edge).
    cyclic: Vec<bool>,
    /// For each component, the nodes reachable from its members by one or more edges.
    bits: Vec<Vec<u64>>,
}

impl Reach {
    /// `edges[i]`: the nodes `i` has an edge to (each below `edges.len()`).
    pub fn new(edges: &[Vec<usize>]) -> Reach {
        let n = edges.len();
        let (comp, members) = components(edges);
        let words = n.div_ceil(64);
        let mut cyclic = vec![false; members.len()];
        let mut bits: Vec<Vec<u64>> = Vec::with_capacity(members.len());
        // Tarjan closes a component after every component it reaches: those bitsets are done
        for (c, nodes) in members.iter().enumerate() {
            let mut set = vec![0u64; words];
            for &u in nodes {
                for &v in &edges[u] {
                    set[v / 64] |= 1 << (v % 64);
                    if comp[v] == c {
                        cyclic[c] = true;
                    } else {
                        for (w, x) in set.iter_mut().zip(&bits[comp[v]]) {
                            *w |= x;
                        }
                    }
                }
            }
            bits.push(set);
        }
        Reach {
            comp,
            members,
            cyclic,
            bits,
        }
    }

    /// Whether `to` is reachable from `from` by one or more edges (`from` itself only on a
    /// cycle).
    pub fn reaches(&self, from: usize, to: usize) -> bool {
        self.bits[self.comp[from]][to / 64] >> (to % 64) & 1 == 1
    }

    /// The components that have a cycle, each as its members in increasing order, ordered by
    /// their first member.
    pub fn cycles(&self) -> Vec<&[usize]> {
        let mut out: Vec<&[usize]> = (0..self.members.len())
            .filter(|&c| self.cyclic[c])
            .map(|c| self.members[c].as_slice())
            .collect();
        out.sort_by_key(|m| m[0]);
        out
    }
}

/// Tarjan's strongly connected components, iteratively: the component of each node, and each
/// component's members (increasing), components in closing order.
fn components(edges: &[Vec<usize>]) -> (Vec<usize>, Vec<Vec<usize>>) {
    const NONE: usize = usize::MAX;
    let n = edges.len();
    let mut index = vec![NONE; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut comp = vec![NONE; n];
    let mut members: Vec<Vec<usize>> = Vec::new();
    let mut next = 0usize;
    // (node, position in its edge list)
    let mut work: Vec<(usize, usize)> = Vec::new();
    for root in 0..n {
        if index[root] != NONE {
            continue;
        }
        work.push((root, 0));
        while let Some(&mut (u, ref mut at)) = work.last_mut() {
            if *at == 0 && index[u] == NONE {
                index[u] = next;
                low[u] = next;
                next += 1;
                stack.push(u);
                on_stack[u] = true;
            }
            if let Some(&v) = edges[u].get(*at) {
                *at += 1;
                if index[v] == NONE {
                    work.push((v, 0));
                } else if on_stack[v] {
                    low[u] = low[u].min(index[v]);
                }
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[u]);
            }
            if low[u] == index[u] {
                let c = members.len();
                let mut nodes = Vec::new();
                loop {
                    let w = stack.pop().expect("u is on the stack");
                    on_stack[w] = false;
                    comp[w] = c;
                    nodes.push(w);
                    if w == u {
                        break;
                    }
                }
                nodes.sort_unstable();
                members.push(nodes);
            }
        }
    }
    (comp, members)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reachability by a walk from every node: the reference.
    fn naive(edges: &[Vec<usize>]) -> Vec<Vec<bool>> {
        let n = edges.len();
        (0..n)
            .map(|s| {
                let mut seen = vec![false; n];
                let mut stack = edges[s].clone();
                while let Some(u) = stack.pop() {
                    if !seen[u] {
                        seen[u] = true;
                        stack.extend(edges[u].iter().copied());
                    }
                }
                seen
            })
            .collect()
    }

    #[test]
    fn matches_a_walk_from_every_node() {
        let mut r = 0x9E37_79B9_7F4A_7C15u64;
        let mut rand = |m: usize| {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            (r % m as u64) as usize
        };
        for case in 0..400 {
            let n = 1 + case % 70;
            let edges: Vec<Vec<usize>> = (0..n)
                .map(|_| (0..rand(4)).map(|_| rand(n)).collect())
                .collect();
            let reach = Reach::new(&edges);
            let want = naive(&edges);
            for i in 0..n {
                for j in 0..n {
                    assert_eq!(reach.reaches(i, j), want[i][j], "{edges:?} {i} {j}");
                }
            }
            for cycle in reach.cycles() {
                for &u in cycle {
                    assert!(want[u][u]);
                    for &v in cycle {
                        assert!(want[u][v]);
                    }
                }
            }
            let on_cycles: usize = reach.cycles().iter().map(|c| c.len()).sum();
            assert_eq!(on_cycles, (0..n).filter(|&u| want[u][u]).count());
        }
    }

    #[test]
    fn a_long_chain_needs_no_deep_stack() {
        let n = 200_000;
        let edges: Vec<Vec<usize>> = (0..n).map(|i| if i > 0 { vec![i - 1] } else { vec![] }).collect();
        let (comp, members) = components(&edges);
        assert_eq!(members.len(), n);
        assert_ne!(comp[0], comp[1]);
    }
}
