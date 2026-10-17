//! IR generation for function bodies, checked via `Body::dump`.

fn dump(src: &str) -> String {
    let p = rv_syn::load_str(src);
    let f = p
        .functions
        .iter()
        .find(|f| f.name == "f")
        .expect("snippet must contain `fn f`");
    f.body.as_ref().expect("f has a body").dump()
}

#[track_caller]
fn check(src: &str, expected: &str) {
    let got = dump(src);
    let expected = expected.trim_start_matches('\n');
    assert_eq!(
        got, expected,
        "\n--- got ---\n{got}\n--- expected ---\n{expected}"
    );
}

#[test]
fn sequence_and_value() {
    check(
        r#"
fn f(a: u32) -> u32 {
    let x = a + 1; // comment
    log::info!("x = {x}");
    v.push(x);
    total += x;
    helper(x)
}"#,
        r#"
[fn] f
  <body>
    let: let x = a + 1
    macro: log::info!("x = {x}")
    method: v.push(x)
    assign: total += x
    call: helper(x) =>
"#,
    );
}

#[test]
fn if_chain_is_one_case_structure() {
    check(
        r#"
fn f() {
    if a { one(); } else if let Some(b) = c { two(b); } else { three(); }
}"#,
        r#"
[fn] f
  <body>
    [if] if =>
      <case a>
        call: one()
      <case let Some(b) = c>
        call: two(b)
      <else>
        call: three()
"#,
    );
}

#[test]
fn match_with_guards_and_try_in_scrutinee() {
    check(
        r#"
fn f() -> Result<(), E> {
    let r = match read()? {
        Some(x) if x > 0 => x,
        Some(_) => { warn(); 0 }
        None => return Ok(()),
    };
    Ok(())
}"#,
        r#"
[fn] f
  <body>
    [match] let r = match read()? ?x1
      <case Some(x) if x > 0>
        expr: x =>
      <case Some(_)>
        call: warn()
        expr: 0 =>
      <case None>
        return: return Ok(()) =>
    call: Ok(()) =>
exit ? from `match read()?` to `f`
exit return from `return Ok(())` to `f`
"#,
    );
}

#[test]
fn labeled_loops_and_exit_targets() {
    check(
        r#"
fn f() {
    'outer: loop {
        for i in 0..n {
            if i == 3 { continue; }
            if done(i) { break 'outer; }
        }
        while let Some(m) = rx.recv() {
            handle(m)?;
        }
    }
}"#,
        r#"
[fn] f
  <body>
    [loop] 'outer: loop
      <body>
        [for] for i in 0..n
          <body>
            [if] if
              <case i == 3>
                continue: continue
            [if] if
              <case done(i)>
                break: break 'outer
        [while] while let Some(m) = rx.recv()
          <body>
            call: handle(m)? ?x1
exit continue from `continue` to `for i in 0..n`
exit break from `break 'outer` to `'outer: loop`
exit ? from `handle(m)?` to `f`
"#,
    );
}

#[test]
fn closures_with_block_bodies_become_structures() {
    check(
        r#"
fn f() {
    let handle = std::thread::Builder::new()
        .name("worker".into())
        .spawn(move || {
            let v = items.iter().map(|x| x + 1).sum::<u32>();
            if v == 0 { return; }
            send(v);
        })
        .map_err(|e| e.to_string())?;
}"#,
        r#"
[fn] f
  <body>
    let: let handle = std::thread::Builder::new().name("worker".into()).spawn(move || {…}).map_err(|e| e.to_string())? ?x1
      [closure] move ||
        <body>
          let: let v = items.iter().map(|x| x + 1).sum::<u32>()
          [if] if
            <case v == 0>
              return: return
          call: send(v)
exit ? from `let handle = std::thread::Builder::new().name("worker".into()).spawn(move || {…}).map_err(|e| e.to_string())?` to `f`
exit return from `return` to `move ||`
"#,
    );
}

#[test]
fn prefixes_let_else_async_and_labeled_blocks() {
    check(
        r#"
fn f() {
    let Some(cfg) = load() else { return; };
    x = if cfg.on { 1 } else { 2 };
    let task = async move { step().await?; };
    let found = 'search: {
        for c in cands { if ok(c) { break 'search Some(c); } }
        None
    };
}"#,
        r#"
[fn] f
  <body>
    let: let Some(cfg) = load() else {…}
      [let-else] else
        <body>
          return: return
    [if] x = if
      <case cfg.on>
        expr: 1 =>
      <else>
        expr: 2 =>
    [async] let task = async move
      <body>
        call: step().await? ?x1
    [block] let found = 'search: { }
      <body>
        [for] for c in cands
          <body>
            [if] if
              <case ok(c)>
                break: break 'search Some(c)
        expr: None =>
exit return from `return` to `f`
exit ? from `step().await?` to `async move`
exit break from `break 'search Some(c)` to `'search: { }`
"#,
    );
}

#[test]
fn spans_point_into_the_source() {
    let src = "fn f() {\n    let x = 1;\n    match x {\n        _ => {}\n    }\n}\n";
    let p = rv_syn::load_str(src);
    let body = p.functions[0].body.as_ref().unwrap();
    let frame = body.node(body.root).children[0];
    let items = &body.node(frame).children;
    let m = body.node(items[1]);
    assert_eq!(p.snippet(&m.head).trim(), "match x");
    assert_eq!(m.span.start.line, 3);
    assert_eq!(m.span.end.line, 5);
    assert_eq!(body.stmt_count(body.root), 1);
}
