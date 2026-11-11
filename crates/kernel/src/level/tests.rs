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

// ## Against brute force
//
// `leq` claims `a <= b` under every assignment of the parameters. These tests
// compare it with evaluating both sides under every assignment up to a bound,
// on many generated pairs (issue #483). Each side is a maximum of `p + k`
// and constants, piecewise by which parameters are zero, so if `a <= b` fails
// anywhere it fails with every parameter at most the total of the successor
// offsets plus two: past that, a parameter only `a` mentions already
// outgrows every term of `b`.

/// The value of `l` when parameter `i` is `env[i]`.
fn value(l: &Level, env: &[u32]) -> u32 {
    match l {
        Level::Zero => 0,
        Level::Succ(l) => value(l, env) + 1,
        Level::Max(a, b) => value(a, env).max(value(b, env)),
        Level::IMax(a, b) => match value(b, env) {
            0 => 0,
            b => value(a, env).max(b),
        },
        Level::Param(p) => env[*p as usize],
    }
}

fn successors(l: &Level) -> u32 {
    match l {
        Level::Zero | Level::Param(_) => 0,
        Level::Succ(l) => successors(l) + 1,
        Level::Max(a, b) | Level::IMax(a, b) => successors(a) + successors(b),
    }
}

/// xorshift64, so the generated cases are the same on every run.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }

    /// A level over `params` parameters, built with the raw constructors so
    /// that `max` and `imax` are not folded before `leq` sees them.
    fn level(&mut self, params: u32, depth: u32) -> Level {
        let leaf = depth == 0 || self.below(3) == 0;
        if leaf {
            return match self.below(3) {
                0 => Level::Zero,
                _ => Level::Param(self.below(params as u64) as u32),
            };
        }
        match self.below(4) {
            0 => self.level(params, depth - 1).succ(),
            1 => Level::Max(Rc::new(self.level(params, depth - 1)), Rc::new(self.level(params, depth - 1))),
            _ => Level::IMax(Rc::new(self.level(params, depth - 1)), Rc::new(self.level(params, depth - 1))),
        }
    }
}

/// `a <= b` under every assignment of `params` parameters to `0..=bound`.
fn holds_everywhere(a: &Level, b: &Level, params: u32, bound: u32) -> Option<Vec<u32>> {
    let mut env = vec![0; params as usize];
    loop {
        if value(a, &env) > value(b, &env) {
            return Some(env);
        }
        let mut i = 0;
        loop {
            if i == env.len() {
                return None;
            }
            if env[i] < bound {
                env[i] += 1;
                break;
            }
            env[i] = 0;
            i += 1;
        }
    }
}

#[test]
fn leq_agrees_with_brute_force() {
    let params = 3;
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (mut held, mut failed) = (0, 0);
    for case in 0..4000 {
        let (a, b) = (rng.level(params, 4), rng.level(params, 4));
        let bound = successors(&a) + successors(&b) + 2;
        let counterexample = holds_everywhere(&a, &b, params, bound);
        let names: Vec<Rc<str>> = ["u", "v", "w"].iter().map(|n| Rc::from(*n)).collect();
        let show = |l: &Level| LevelDisplay { level: l, names: &names }.to_string();
        match (a.leq(&b), counterexample) {
            (true, Some(env)) => panic!("case {case}: leq says `{} <= {}`, but it fails at {env:?}", show(&a), show(&b)),
            (false, None) => panic!("case {case}: leq rejects `{} <= {}`, which holds up to {bound}", show(&a), show(&b)),
            (true, None) => held += 1,
            (false, Some(_)) => failed += 1,
        }
    }
    // Both answers must be exercised, or the generator is too narrow.
    assert!(held > 400 && failed > 400, "held {held}, failed {failed}");
}
