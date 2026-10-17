use std::path::{Path, PathBuf};

use rv_ir::{FnOwner, ParamMode, Project, Severity, TargetKind};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn fn_paths(p: &Project) -> Vec<&str> {
    let mut v: Vec<&str> = p.functions.iter().map(|f| f.qual_path.as_str()).collect();
    v.sort();
    v
}

#[test]
fn module_tree_and_functions() {
    let p = rv_syn::load(&fixture("modcrate")).unwrap();
    assert_eq!(p.packages.len(), 1);
    let pkg = &p.packages[0];
    let kinds: Vec<_> = pkg
        .targets
        .iter()
        .map(|t| (t.kind, t.name.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [(TargetKind::Lib, "modcrate"), (TargetKind::Bin, "modcrate")]
    );

    assert_eq!(
        fn_paths(&p),
        [
            "modcrate::S::fmt",
            "modcrate::S::method",
            "modcrate::Tr::provided",
            "modcrate::dir::in_dir",
            "modcrate::dir::inner::in_inner",
            "modcrate::flat::child::in_child",
            "modcrate::flat::in_flat",
            "modcrate::free_fn",
            "modcrate::free_fn::helper",
            "modcrate::inline_mod::deeper::in_deeper",
            "modcrate::inline_mod::in_inline",
            "modcrate::main",
            "modcrate::not_test",
            "modcrate::renamed::in_custom",
        ]
    );

    let fmt = p.functions.iter().find(|f| f.name == "fmt").unwrap();
    assert_eq!(
        fmt.owner,
        FnOwner::Impl {
            self_ty: "S<T>".into(),
            trait_: Some("std::fmt::Display".into())
        }
    );
    assert_eq!(
        fmt.owner.group_label().unwrap(),
        "impl std::fmt::Display for S<T>"
    );

    let method = p.functions.iter().find(|f| f.name == "method").unwrap();
    assert!(method.sig.is_async);
    assert_eq!(method.sig.params[0].mode, ParamMode::Ref);
}

#[test]
fn spans_and_signatures() {
    let p = rv_syn::load(&fixture("modcrate")).unwrap();
    let f = p.functions.iter().find(|f| f.name == "free_fn").unwrap();
    assert_eq!(f.span.start.line, 12);
    assert_eq!(f.span.end.line, 16);
    assert_eq!(p.snippet(&f.name_span), "free_fn");
    assert_eq!(f.name_span.start.col, 7);
    assert_eq!(f.sig.text, "fn free_fn(a: &mut Vec<u8>, b: i32) -> usize");
    assert_eq!(f.sig.params[0].pat, "a");
    assert_eq!(f.sig.params[0].ty, "&mut Vec<u8>");
    assert_eq!(f.sig.params[0].mode, ParamMode::RefMut);
    assert_eq!(f.sig.params[1].mode, ParamMode::Value);
    assert_eq!(f.sig.ret.as_deref(), Some("usize"));
    assert!(p.snippet(f.body_span.as_ref().unwrap()).starts_with('{'));

    let helper = p.functions.iter().find(|f| f.name == "helper").unwrap();
    assert!(matches!(helper.owner, FnOwner::Nested { .. }));
}

#[test]
fn tests_are_counted_not_shown() {
    let p = rv_syn::load(&fixture("modcrate")).unwrap();
    let t = p.packages[0].tests;
    assert_eq!(t.cfg_test_modules, 1);
    assert_eq!(t.test_targets, 1);
    // 2 in `mod tests`, 2 in tests/it.rs
    assert_eq!(t.test_fns, 4);
    assert!(
        p.functions
            .iter()
            .all(|f| f.name != "a" && f.name != "it_works")
    );
}

#[test]
fn unlinked_and_missing_files() {
    let p = rv_syn::load(&fixture("modcrate")).unwrap();
    let unlinked: Vec<_> = p.packages[0]
        .unlinked_files
        .iter()
        .map(|&id| {
            p.file(id)
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(unlinked, ["orphan.rs"]);
    assert!(
        p.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Warning && d.message.contains("mod missing;"))
    );
}

#[test]
fn single_file_with_bom_and_shebang() {
    let dir = std::env::temp_dir().join(format!("rv-syn-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("script.rs");
    std::fs::write(
        &path,
        "\u{feff}#!/usr/bin/env run-cargo-script\nfn ümlaut_fn() {}\n",
    )
    .unwrap();

    let p = rv_syn::load(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();

    assert!(p.files[0].parse_error.is_none());
    let f = &p.functions[0];
    assert_eq!(f.name, "ümlaut_fn");
    assert_eq!(p.snippet(&f.name_span), "ümlaut_fn");
    assert_eq!(f.span.start.line, 2);
}

#[test]
fn parse_errors_keep_text() {
    let dir = std::env::temp_dir().join(format!("rv-syn-err-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("broken.rs");
    std::fs::write(&path, "fn ok() {}\nfn broken( {\n").unwrap();

    let p = rv_syn::load(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();

    assert!(p.files[0].parse_error.is_some());
    assert!(p.files[0].text.contains("broken"));
    assert!(p.diagnostics.iter().any(|d| d.severity == Severity::Error));
}
