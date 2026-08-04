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
