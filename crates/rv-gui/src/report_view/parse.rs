//! Markdown of the text outputs as blocks: headings, list items, tables
//! and paragraphs, with `file:line` locations and function names resolved
//! to link targets.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rv_ir::{FileId, FnId, Project};

pub(super) enum BlockKind {
    /// Rows of cells; the first row is the head.
    Table(Vec<Vec<Vec<Seg>>>),
    Title,
    Heading,
    Subheading,
    /// List item with indent level.
    Item(usize),
    Paragraph,
    Gap,
}

pub(super) struct Block {
    pub kind: BlockKind,
    pub segs: Vec<Seg>,
}

pub(super) enum Seg {
    Text(String),
    Bold(String),
    Code(String, Option<FnId>),
    Loc(String, FileId, u32),
}

/// The directory paths in the text are relative to.
pub(super) fn base_dir(root: &Path) -> PathBuf {
    if root.is_file() {
        root.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        root.to_path_buf()
    }
}

/// Lookup tables for links.
struct Targets {
    /// Path relative to the project directory (with `/`) -> file.
    files: HashMap<String, FileId>,
    /// Qualified path and unique `Type::name` / `module::name` suffixes -> function.
    functions: HashMap<String, Option<FnId>>,
}

impl Targets {
    fn new(project: &Project) -> Self {
        let base = base_dir(&project.root);
        let files = project
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let rel = f.path.strip_prefix(&base).unwrap_or(&f.path);
                (rel.to_string_lossy().replace('\\', "/"), FileId(i as u32))
            })
            .collect();
        let mut functions: HashMap<String, Option<FnId>> = HashMap::new();
        for (i, f) in project.functions.iter().enumerate() {
            let id = FnId(i as u32);
            functions.insert(f.qual_path.clone(), Some(id));
            let parts: Vec<&str> = f.qual_path.rsplitn(3, "::").collect();
            if let [last, prev, _] = parts.as_slice() {
                // Ambiguous short names get no link.
                functions
                    .entry(format!("{prev}::{last}"))
                    .and_modify(|e| {
                        if *e != Some(id) {
                            *e = None
                        }
                    })
                    .or_insert(Some(id));
            }
        }
        // Full paths win over equal short names of other functions.
        for (i, f) in project.functions.iter().enumerate() {
            functions.insert(f.qual_path.clone(), Some(FnId(i as u32)));
        }
        Targets { files, functions }
    }
}

/// The blocks of a Markdown text.
pub(super) fn parse(project: &Project, text: &str) -> Vec<Block> {
    let targets = Targets::new(project);
    let mut blocks = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            blocks.push(Block {
                kind: BlockKind::Paragraph,
                segs: vec![Seg::Code(line.to_string(), None)],
            });
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with('|') {
            table_row(trimmed, &targets, &mut blocks);
            continue;
        }
        let indent = (line.len() - trimmed.len()) / 2;
        let (kind, rest) = if let Some(r) = line.strip_prefix("### ") {
            (BlockKind::Subheading, r)
        } else if let Some(r) = line.strip_prefix("## ") {
            (BlockKind::Heading, r)
        } else if let Some(r) = line.strip_prefix("# ") {
            (BlockKind::Title, r)
        } else if let Some(r) = trimmed.strip_prefix("- ") {
            (BlockKind::Item(indent), r)
        } else if trimmed.is_empty() {
            (BlockKind::Gap, "")
        } else {
            (BlockKind::Paragraph, trimmed)
        };
        blocks.push(Block {
            kind,
            segs: inline(rest, &targets),
        });
    }
    blocks
}

/// Adds a `| a | b |` line to the table at the end of `blocks`, starting
/// one if needed; the `|---|---|` line below the head is left out.
fn table_row(line: &str, targets: &Targets, blocks: &mut Vec<Block>) {
    let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
    let is_rule = |c: &&str| !c.is_empty() && c.chars().all(|ch| "-: ".contains(ch));
    if cells.iter().all(is_rule) {
        return;
    }
    let row = cells.iter().map(|c| inline(c, targets)).collect();
    match blocks.last_mut() {
        Some(Block {
            kind: BlockKind::Table(rows),
            ..
        }) => rows.push(row),
        _ => blocks.push(Block {
            kind: BlockKind::Table(vec![row]),
            segs: Vec::new(),
        }),
    }
}

/// Splits a line into text, `**bold**`, `` `code` `` and `file.rs:12, 40` locations.
fn inline(text: &str, targets: &Targets) -> Vec<Seg> {
    let mut segs = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let code = rest.find('`');
        let bold = rest.find("**");
        match (code, bold) {
            (Some(c), b) if b.is_none_or(|b| c < b) => {
                locations(&rest[..c], targets, &mut segs);
                let after = &rest[c + 1..];
                let end = after.find('`').unwrap_or(after.len());
                let code = &after[..end];
                let f = targets.functions.get(code).copied().flatten();
                segs.push(Seg::Code(code.to_string(), f));
                rest = after.get(end + 1..).unwrap_or("");
            }
            (_, Some(b)) => {
                locations(&rest[..b], targets, &mut segs);
                let after = &rest[b + 2..];
                let end = after.find("**").unwrap_or(after.len());
                let inner = &after[..end];
                // `**`code`**`: keep the code (and its link), shown as is.
                if inner.starts_with('`') {
                    segs.extend(inline(inner, targets));
                } else {
                    segs.push(Seg::Bold(inner.to_string()));
                }
                rest = after.get(end + 2..).unwrap_or("");
            }
            _ => {
                locations(rest, targets, &mut segs);
                rest = "";
            }
        }
    }
    segs
}

/// Plain text with `path.rs:12` and following `, 40, 41` line numbers as links.
fn locations(text: &str, targets: &Targets, segs: &mut Vec<Seg>) {
    let mut plain = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(".rs:") {
        let start = rest[..i]
            .rfind(|c: char| c.is_whitespace() || c == '(')
            .map_or(0, |s| s + 1);
        let digits = rest[i + 4..]
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len() - i - 4);
        let path = &rest[start..i + 3];
        let (Some(&file), Ok(line)) = (
            targets.files.get(path),
            rest[i + 4..i + 4 + digits].parse::<u32>(),
        ) else {
            plain.push_str(&rest[..i + 4]);
            rest = &rest[i + 4..];
            continue;
        };
        plain.push_str(&rest[..start]);
        push_text(segs, &mut plain);
        segs.push(Seg::Loc(
            rest[start..i + 4 + digits].to_string(),
            file,
            line,
        ));
        rest = &rest[i + 4 + digits..];
        // More lines of the same file: `(src/a.rs:12, 40, 41)`.
        while let Some(more) = rest.strip_prefix(", ") {
            let n = more
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(more.len());
            let Ok(line) = more[..n].parse::<u32>() else {
                break;
            };
            segs.push(Seg::Text(", ".into()));
            segs.push(Seg::Loc(more[..n].to_string(), file, line));
            rest = &more[n..];
        }
    }
    plain.push_str(rest);
    push_text(segs, &mut plain);
}

fn push_text(segs: &mut Vec<Seg>, plain: &mut String) {
    if !plain.is_empty() {
        segs.push(Seg::Text(std::mem::take(plain)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_and_code_become_links() {
        let p = rv_syn::load_str("fn main() {}\nfn helfer() {}\n");
        let targets = Targets::new(&p);
        let segs = inline(
            "**`main`** at snippet.rs:1, 2 in `snippet::helfer` (other.rs:3)",
            &targets,
        );
        let shown: Vec<String> = segs
            .iter()
            .map(|s| match s {
                Seg::Text(t) => format!("T({t})"),
                Seg::Bold(t) => format!("B({t})"),
                Seg::Code(t, f) => format!("C({t},{})", f.is_some()),
                Seg::Loc(t, _, l) => format!("L({t}={l})"),
            })
            .collect();
        assert_eq!(
            shown,
            [
                "C(main,false)",
                "T( at )",
                "L(snippet.rs:1=1)",
                "T(, )",
                "L(2=2)",
                "T( in )",
                "C(snippet::helfer,true)",
                "T( (other.rs:3))",
            ]
        );
    }

    #[test]
    fn table_lines_become_one_table_without_the_rule() {
        let p = rv_syn::load_str(
            "fn main() {}
",
        );
        let blocks = parse(
            &p,
            "| a | b |
|---|---|
| 1 | `main` |
text",
        );
        let BlockKind::Table(rows) = &blocks[0].kind else {
            panic!("no table");
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].len(), 2);
        assert!(matches!(blocks[1].kind, BlockKind::Paragraph));
    }
}
