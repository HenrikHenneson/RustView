//! Data flow on small snippets, checked via `DataFlow::dump`.

fn flow(src: &str) -> String {
    let p = rv_syn::load_str(src);
    let f = p.functions.iter().find(|f| f.name == "f").expect("fn f");
    let body = f.body.as_ref().unwrap();
    rv_analysis::data_flow(body).dump(body)
}

#[track_caller]
fn check(src: &str, expected: &str) {
    let got = flow(src);
    let expected = expected.trim_start_matches('\n');
    assert_eq!(
        got, expected,
        "\n--- got ---\n{got}--- expected ---\n{expected}"
    );
}

#[test]
fn def_use_params_and_shadowing() {
    check(
        r#"
fn f(a: u32) -> u32 {
    let x = a + 1;
    let y = helper(x);
    let x = x * 2;
    y + x
}"#,
        r#"
a: f -> let x = a + 1
x: let x = a + 1 -> let x = x * 2
x: let x = a + 1 -> let y = helper(x)
x: let x = x * 2 -> y + x
y: let y = helper(x) -> y + x
"#,
    );
}

#[test]
fn mutations_order_statements() {
    check(
        r#"
fn f() {
    let mut v = Vec::new();
    let mut s = Session::new();
    s.step();
    v.push(1);
    fill(&mut v);
    v[0] = 5;
    total += v.len();
    show(&v);
}"#,
        r#"
s: let mut s = Session::new() -> s.step() [mut?]
v: fill(&mut v) -> v[0] = 5 [mut]
v: let mut v = Vec::new() -> v.push(1) [mut]
v: v.push(1) -> fill(&mut v) [mut]
v: v[0] = 5 -> show(&v)
v: v[0] = 5 -> total += v.len()
"#,
    );
}

#[test]
fn structures_are_writers_afterwards() {
    check(
        r#"
fn f(items: &[u32]) -> u32 {
    let mut sum = 0;
    for it in items {
        if *it > 10 { sum += it; }
    }
    let n = items.len();
    sum / n as u32
}"#,
        r#"
it: <body> -> *it > 10
it: <body> -> sum += it
items: f -> for it in items
items: f -> let n = items.len()
n: let n = items.len() -> sum / n as u32
sum: for it in items -> sum / n as u32
sum: let mut sum = 0 -> sum += it [mut]
"#,
    );
}

#[test]
fn closures_capture_and_shadow() {
    check(
        r#"
fn f(rx: Receiver<u32>) {
    let limit = 5;
    let total = items.iter().map(|limit| limit + 1).sum::<u32>();
    let h = std::thread::spawn(move || {
        while let Ok(v) = rx.recv() {
            if v > limit { report(v, total); }
        }
    });
}"#,
        r#"
limit: let limit = 5 -> v > limit
rx: f -> while let Ok(v) = rx.recv()
total: let total = items.iter().map(|limit| limit + 1).sum::<u32>() -> report(v, total)
v: <body> -> report(v, total)
v: <body> -> v > limit
"#,
    );
}

#[test]
fn match_arms_and_macros() {
    check(
        r#"
fn f(cmd: Cmd, name: &str) {
    let count = 3;
    match cmd {
        Cmd::Go(speed) if speed > count => log::info!("go {speed} {}", name),
        Cmd::Stop => println!("{count:>width$}", width = 4),
        _ => my_macro!(count, other.field),
    }
}"#,
        r#"
cmd: f -> match cmd
count: let count = 3 -> Cmd::Go(speed) if speed > count
count: let count = 3 -> my_macro!(count, other.field) (guess)
count: let count = 3 -> println!("{count:>width$}", width = 4)
name: f -> log::info!("go {speed} {}", name)
speed: Cmd::Go(speed) if speed > count -> log::info!("go {speed} {}", name)
"#,
    );
}
