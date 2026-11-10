use super::*;

fn u() -> Level {
    Level::Param(0)
}

fn v() -> Level {
    Level::Param(1)
}

#[test]
fn constants_compare_numerically() {
    assert!(Level::constant(1).leq(&Level::constant(2)));
    assert!(!Level::constant(2).leq(&Level::constant(1)));
    assert!(Level::Zero.is_zero());
    assert!(Level::constant(1).is_nonzero());
}

#[test]
fn parameters_range_over_every_level() {
    assert!(u().leq(&u().succ()));
    assert!(!u().succ().leq(&u()));
    assert!(!u().leq(&v()));
    assert!(u().leq(&Level::max(&u(), &v())));
    assert!(Level::constant(1).leq(&Level::max(&Level::constant(1), &u())));
    assert!(!Level::constant(1).leq(&u()));
    assert!(!u().is_zero() && !u().is_nonzero());
}

#[test]
fn imax_is_zero_on_a_zero_codomain() {
    assert!(Level::imax(&u(), &Level::Zero).is_zero());
    assert!(Level::imax(&u(), &v().succ()).equal(&Level::max(&u(), &v().succ())));
}

#[test]
fn imax_case_splits_on_its_codomain() {
    let imax = Level::imax(&u(), &v());
    // `imax u v <= max u v` holds whether `v` is zero or not.
    assert!(imax.leq(&Level::max(&u(), &v())));
    // `max u v <= imax u v` fails at `v = 0, u = 1`.
    assert!(!Level::max(&u(), &v()).leq(&imax));
    assert!(v().leq(&imax));
    assert!(Level::imax(&u(), &u()).equal(&u()));
}

#[test]
fn nested_imax_distributes() {
    let w = Level::Param(2);
    let lhs = Level::imax(&u(), &Level::imax(&v(), &w));
    let rhs = Level::max(&Level::imax(&u(), &w), &Level::imax(&v(), &w));
    assert!(lhs.equal(&rhs));
}

#[test]
fn display_folds_successors() {
    let names: Vec<Rc<str>> = vec!["u".into()];
    let shown = LevelDisplay { level: &u().succ().succ(), names: &names }.to_string();
    assert_eq!(shown, "u + 2");
    let shown = LevelDisplay { level: &Level::constant(3), names: &names }.to_string();
    assert_eq!(shown, "3");
}
