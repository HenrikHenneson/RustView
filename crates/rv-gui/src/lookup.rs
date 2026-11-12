//! Finding functions and source lines in a project.

use rv_ir::{FileId, FnId, NodeId, Project};

/// Exact qualified path, or the shortest one ending in `::wanted`.
pub fn find_function(project: &Project, wanted: &str) -> Option<FnId> {
    let suffix = format!("::{wanted}");
    project
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| f.qual_path == wanted || f.qual_path.ends_with(&suffix))
        .min_by_key(|(_, f)| f.qual_path.len())
        .map(|(i, _)| FnId(i as u32))
}

/// The function with this qualified path. The same path can exist twice
/// (e.g. a file in two targets): then the one closest to `line`.
pub fn function_near(project: &Project, qual_path: &str, line: u32) -> Option<FnId> {
    project
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| f.qual_path == qual_path)
        .min_by_key(|(_, f)| f.span.start.line.abs_diff(line))
        .map(|(i, _)| FnId(i as u32))
}

/// The innermost function around a line of a file, if any.
pub fn innermost_function_at(project: &Project, file: FileId, line: u32) -> Option<FnId> {
    project
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            f.span.file == file && (f.span.start.line..=f.span.end.line).contains(&line)
        })
        .min_by_key(|(_, f)| f.span.line_count())
        .map(|(i, _)| FnId(i as u32))
}

/// Line on which a statement of a function starts (0 for a function without body).
pub fn node_line(project: &Project, f: FnId, node: NodeId) -> u32 {
    project
        .function(f)
        .body
        .as_ref()
        .map_or(0, |b| b.node(node).span.start.line)
}
