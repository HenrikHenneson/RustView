use super::*;

const METRICS: Metrics = Metrics {
    char_width: 7.0,
    line_height: 14.0,
    max_label_chars: 60,
};

const SRC: &str = r#"
fn f(rx: Receiver<u32>) -> Result<(), E> {
    let mut total = 0;
    'outer: loop {
        match rx.recv()? {
            0 => break 'outer,
            1 => continue,
            2 => { total += 1; }
            3 => { total += 2; }
            n => {
                for i in 0..n {
                    if i % 2 == 0 { total += i; } else { log::info!("odd {i}"); }
                }
            }
        }
    }
    std::thread::spawn(move || {
        work(total);
    });
    Ok(())
}
"#;

fn body() -> Body {
    let p = rv_syn::load_str(SRC);
    p.functions[0].body.clone().unwrap()
}

fn all_expanded(body: &Body) -> HashSet<NodeId> {
    (0..body.nodes.len() as u32).map(NodeId).collect()
}

fn check_nesting(body: &Body, l: &Layout) {
    for item in &l.items {
        let node = body.node(item.node);
        if let Some(parent) = node.parent.and_then(|p| l.item(p)) {
            assert!(
                parent.rect.expand(0.01).contains_rect(item.rect),
                "{:?} {:?} not inside parent {:?}",
                node.label,
                item.rect,
                parent.rect
            );
        }
        // Siblings must not overlap.
        let siblings: Vec<&Item> = node.children.iter().filter_map(|&c| l.item(c)).collect();
        for (i, a) in siblings.iter().enumerate() {
            for b in &siblings[i + 1..] {
                let overlap = a.rect.intersect(b.rect);
                assert!(
                    overlap.width() <= 0.01 || overlap.height() <= 0.01,
                    "siblings overlap: {:?} {:?}",
                    body.node(a.node).label,
                    body.node(b.node).label
                );
            }
        }
    }
}

#[test]
fn everything_nested_and_disjoint_in_both_orientations() {
    let body = body();
    let mut state = ViewState::default();
    state.all_cases.insert(NodeId(0), true);
    for orientation in [Orientation::Vertical, Orientation::Horizontal] {
        let l = layout(&body, &state, &all_expanded(&body), orientation, METRICS);
        check_nesting(&body, &l);
        assert!(l.items.iter().all(|i| {
            Rect::from_min_size(Pos2::ZERO, l.size)
                .expand(0.01)
                .contains_rect(i.rect)
        }));
    }
}

#[test]
fn sequence_follows_orientation() {
    let body = body();
    let frame = body.node(body.root).children[0];
    let items = &body.node(frame).children;
    for (orientation, vertical) in [
        (Orientation::Vertical, true),
        (Orientation::Horizontal, false),
    ] {
        let l = layout(
            &body,
            &ViewState::default(),
            &all_expanded(&body),
            orientation,
            METRICS,
        );
        let rects: Vec<Rect> = items.iter().map(|&i| l.item(i).unwrap().rect).collect();
        for w in rects.windows(2) {
            if vertical {
                assert!(w[1].min.y >= w[0].max.y);
            } else {
                assert!(w[1].min.x >= w[0].max.x);
            }
        }
    }
}

#[test]
fn collapsed_structures_hide_their_content() {
    let body = body();
    let only_root = HashSet::from([body.root]);
    let l = layout(
        &body,
        &ViewState::default(),
        &only_root,
        Orientation::Vertical,
        METRICS,
    );
    let frame = body.node(body.root).children[0];
    // Root frame items are visible, nothing below the loop.
    let lp = body.node(frame).children[1];
    let item = l.item(lp).unwrap();
    let ItemKind::Structure(s) = &item.kind else {
        panic!()
    };
    assert!(!s.expanded);
    assert!(s.info.as_ref().unwrap().text.contains("stmts"));
    assert!(body.node(lp).children.iter().all(|&c| l.item(c).is_none()));
    // Header only.
    assert_eq!(item.rect.height(), METRICS.line_height + 2.0 * PAD);
}

#[test]
fn many_cases_show_one_at_a_time() {
    let body = body();
    let l = layout(
        &body,
        &ViewState::default(),
        &all_expanded(&body),
        Orientation::Vertical,
        METRICS,
    );
    let m = (0..body.nodes.len() as u32)
        .map(NodeId)
        .find(|&n| body.node(n).label.starts_with("match"))
        .unwrap();
    let ItemKind::Structure(s) = &l.item(m).unwrap().kind else {
        panic!()
    };
    let sel = s.selector.as_ref().unwrap();
    assert!(!sel.showing_all);
    assert_eq!((sel.index, sel.count), (0, 5));
    let cases = case_frames(&body, m);
    assert!(l.item(cases[0]).is_some());
    assert!(cases[1..].iter().all(|&c| l.item(c).is_none()));

    let mut state = ViewState::default();
    state.case_index.insert(m, 4);
    let l = layout(
        &body,
        &state,
        &all_expanded(&body),
        Orientation::Vertical,
        METRICS,
    );
    assert!(l.item(cases[4]).is_some() && l.item(cases[0]).is_none());
}

#[test]
fn budget_expands_outside_in() {
    let body = body();
    let state = ViewState::default();
    let small = default_expanded(&body, &state, 3);
    assert_eq!(small, HashSet::from([body.root]));
    let large = default_expanded(&body, &state, 1000);
    // Everything reachable is expanded with a generous budget.
    let structures = body
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Structure { .. }))
        .count();
    // The match shows one case at a time, so structures in hidden cases stay closed.
    assert!(large.len() < structures);
    assert!(large.len() >= 4);
}

#[test]
fn exits_are_summarized_at_their_target() {
    let body = body();
    let l = layout(
        &body,
        &ViewState::default(),
        &all_expanded(&body),
        Orientation::Vertical,
        METRICS,
    );
    let ItemKind::Structure(root) = &l.item(body.root).unwrap().kind else {
        panic!()
    };
    assert_eq!(root.info.as_ref().unwrap().text, "exits: ?×1");
    let lp = (0..body.nodes.len() as u32)
        .map(NodeId)
        .find(|&n| body.node(n).label.contains("'outer: loop"))
        .unwrap();
    let ItemKind::Structure(s) = &l.item(lp).unwrap().kind else {
        panic!()
    };
    assert_eq!(s.info.as_ref().unwrap().text, "exits: break×1 continue×1");
}
