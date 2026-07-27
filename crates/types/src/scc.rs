//! Tarjan's strongly-connected-components algorithm.
//!
//! Global definitions may reference each other, including mutual recursion and
//! forward references. To type them the way Hindley-Milner intends, we group the
//! definitions into strongly-connected components (each mutually-recursive
//! cluster is one component), then process the components in dependency order:
//! a component is checked and generalized before any component that uses it, so
//! polymorphism flows across the boundaries while recursion stays monomorphic
//! within a component.
//!
//! [`scc`] returns the components in reverse topological order of the
//! condensation, which is exactly "dependencies first": if component A depends
//! on component B, B appears before A. That is the natural output order of
//! Tarjan's algorithm.

/// Compute the strongly-connected components of a directed graph given as an
/// adjacency list (`graph[v]` lists the successors of vertex `v`). Components
/// come out in reverse topological order (a component precedes the ones that
/// depend on it).
pub fn scc(graph: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut state = Tarjan {
        graph,
        index: vec![NONE; graph.len()],
        lowlink: vec![0; graph.len()],
        on_stack: vec![false; graph.len()],
        stack: Vec::new(),
        next_index: 0,
        components: Vec::new(),
    };
    for v in 0..graph.len() {
        if state.index[v] == NONE {
            state.connect(v);
        }
    }
    state.components
}

const NONE: usize = usize::MAX;

struct Tarjan<'a> {
    graph: &'a [Vec<usize>],
    /// Depth-first discovery order of each vertex (`NONE` until visited).
    index: Vec<usize>,
    /// The smallest index reachable from a vertex (the invariant Tarjan tracks).
    lowlink: Vec<usize>,
    on_stack: Vec<bool>,
    stack: Vec<usize>,
    next_index: usize,
    components: Vec<Vec<usize>>,
}

impl Tarjan<'_> {
    fn connect(&mut self, v: usize) {
        self.index[v] = self.next_index;
        self.lowlink[v] = self.next_index;
        self.next_index += 1;
        self.stack.push(v);
        self.on_stack[v] = true;

        for &w in &self.graph[v] {
            if self.index[w] == NONE {
                self.connect(w);
                self.lowlink[v] = self.lowlink[v].min(self.lowlink[w]);
            } else if self.on_stack[w] {
                self.lowlink[v] = self.lowlink[v].min(self.index[w]);
            }
        }

        // A vertex whose lowlink equals its own index is the root of an SCC;
        // everything above it on the stack forms the component.
        if self.lowlink[v] == self.index[v] {
            let mut component = Vec::new();
            loop {
                let w = self.stack.pop().expect("stack holds the component members");
                self.on_stack[w] = false;
                component.push(w);
                if w == v {
                    break;
                }
            }
            self.components.push(component);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::scc;

    fn sorted(mut comps: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
        for c in &mut comps {
            c.sort();
        }
        comps
    }

    #[test]
    fn singletons_in_dependency_order() {
        // 0 -> 1 -> 2, no cycles: three singletons, dependencies first.
        let g = vec![vec![1], vec![2], vec![]];
        assert_eq!(scc(&g), vec![vec![2], vec![1], vec![0]]);
    }

    #[test]
    fn a_two_cycle_is_one_component() {
        // 0 <-> 1 (mutual recursion) and a separate 2 they both use.
        let g = vec![vec![1, 2], vec![0], vec![]];
        let comps = sorted(scc(&g));
        assert_eq!(comps, vec![vec![2], vec![0, 1]]);
    }

    #[test]
    fn self_loop_is_its_own_component() {
        let g = vec![vec![0]];
        assert_eq!(scc(&g), vec![vec![0]]);
    }

    #[test]
    fn dependency_precedes_dependent() {
        // 0 -> 1, so component {1} must come before {0}.
        let g = vec![vec![1], vec![]];
        let comps = scc(&g);
        let pos0 = comps.iter().position(|c| c.contains(&0)).unwrap();
        let pos1 = comps.iter().position(|c| c.contains(&1)).unwrap();
        assert!(pos1 < pos0);
    }
}
