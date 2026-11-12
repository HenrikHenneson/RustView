//! Code health: measures of functions, module cycles and the score.

use rv_analysis::health::{Health, Level, Limit, Limits, Measure, complexity, nesting};
use rv_analysis::{CallGraph, health_report};

fn body_of(src: &str) -> rv_ir::Body {
    let p = rv_syn::load_str(src);
    p.functions[0].body.clone().expect("body")
}

#[test]
fn flat_code_has_no_complexity() {
    let body = body_of("fn f(a: u32) -> u32 { let b = a + 1; let c = b * 2; c }");
    assert_eq!(complexity(&body), 0);
    assert_eq!(nesting(&body), 0);
}

#[test]
fn branches_count_more_the_deeper_they_are() {
    // for: 1, if inside it: 2 (+1 for the else), match inside the if: 3.
    let body = body_of(
        "fn f(v: Vec<u32>) {
            for x in v {
                if x > 1 {
                    match x { 2 => a(), _ => b() }
                } else {
                    c();
                }
            }
        }",
    );
    assert_eq!(complexity(&body), 1 + 3 + 3);
    assert_eq!(nesting(&body), 3);
}

#[test]
fn closures_deepen_but_do_not_count() {
    let body = body_of("fn f() { run(|| { if x() { y(); } }); }");
    assert_eq!(complexity(&body), 2);
    assert_eq!(nesting(&body), 2);
}

#[test]
fn limit_levels() {
    let limit = Limit { target: 4, max: 6 };
    assert_eq!(limit.level(4), Level::Ok);
    assert_eq!(limit.level(5), Level::OverTarget);
    assert_eq!(limit.level(6), Level::OverTarget);
    assert_eq!(limit.level(7), Level::OverLimit);
}

const CYCLE: &str = "
mod a {
    pub fn start() { crate::b::step(); }
    pub fn end() {}
}
mod b {
    pub fn step() { crate::a::end(); crate::c::leaf(); }
}
mod c {
    pub fn leaf() {}
}
fn many(a: u8, b: u8, c: u8, d: u8, e: u8, f: u8, g: u8, h: u8) {}
";

#[test]
fn finds_module_cycles_and_their_links() {
    let p = rv_syn::load_str(CYCLE);
    let health = Health::new(&p, &CallGraph::new(&p));
    assert_eq!(health.cycles.len(), 1);
    let names: Vec<String> = health.cycles[0]
        .modules
        .iter()
        .map(|&m| p.module(m).qual_name())
        .collect();
    assert_eq!(names, ["snippet::a", "snippet::b"]);
    assert_eq!(health.cycles[0].links.len(), 2);
    assert_eq!(health.modules_in_cycles(), 2);
}

#[test]
fn score_and_report() {
    let p = rv_syn::load_str(CYCLE);
    let health = Health::new(&p, &CallGraph::new(&p));
    let worst = health.functions[0];
    assert_eq!(p.function(worst.f).name, "many");
    assert_eq!(worst.value(Measure::Params), 8);
    assert_eq!(health.fn_level(&worst), Level::OverLimit);
    // One function over a limit and one cycle.
    assert_eq!(health.violations(), 2);
    let score = health.score();
    assert!(score.functions < 100 && score.modules < 100, "{score:?}");
    assert_eq!(score.files, 100);

    let text = health_report(&p, &health);
    for expected in [
        "# RustView code health",
        "### Over a limit (1)",
        "`snippet::many` (snippet.rs:12): 1 lines, **8 parameters**",
        "- `snippet::a`, `snippet::b`",
        "`snippet::a` → `snippet::b`: 1 call, e.g. snippet.rs:3 in `start`",
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }
}

#[test]
fn healthy_code_scores_full() {
    let p = rv_syn::load_str("fn main() { helper(); }\nfn helper() {}");
    let health = Health::with_limits(&p, &CallGraph::new(&p), Limits::default());
    assert_eq!(health.violations(), 0);
    assert_eq!(health.score().total, 100);
    assert_eq!(health.score().grade(), 'A');
    assert!(health_report(&p, &health).contains("None: no group"));
}
