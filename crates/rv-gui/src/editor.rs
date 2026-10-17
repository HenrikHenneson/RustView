//! Opening a source location in an external editor.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

pub const DEFAULT_TEMPLATE: &str = "code --goto {file}:{line}:{col}";

/// A command line template with `{file}`, `{line}` and `{col}` placeholders.
#[derive(Clone, Debug)]
pub struct EditorCommand {
    args: Vec<String>,
}

impl EditorCommand {
    pub fn parse(template: &str) -> Result<Self> {
        let args = split_args(template);
        if args.is_empty() {
            bail!("empty editor command");
        }
        Ok(EditorCommand { args })
    }

    /// `line` is 1-based, `col` 0-based (converted to the 1-based editor convention).
    pub fn open(&self, file: &Path, line: u32, col: u32) -> Result<()> {
        let file = file.to_string_lossy();
        let (line, col) = (line.max(1).to_string(), (col + 1).to_string());
        let args: Vec<String> = self
            .args
            .iter()
            .map(|a| {
                a.replace("{file}", &file)
                    .replace("{line}", &line)
                    .replace("{col}", &col)
            })
            .collect();
        let program = find_program(&args[0]);
        let mut cmd = Command::new(&program);
        cmd.args(&args[1..]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd.spawn()
            .with_context(|| format!("starting {}", program.display()))?;
        Ok(())
    }
}

/// Whitespace split that keeps "double quoted" parts together.
fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut has_arg = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                has_arg = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has_arg {
                    args.push(std::mem::take(&mut cur));
                    has_arg = false;
                }
            }
            c => {
                cur.push(c);
                has_arg = true;
            }
        }
    }
    if has_arg {
        args.push(cur);
    }
    args
}

/// On Windows `code` is `code.cmd`, which `Command` does not find by itself.
fn find_program(name: &str) -> PathBuf {
    let path = Path::new(name);
    if !cfg!(windows) || path.components().count() > 1 || path.extension().is_some() {
        return path.to_path_buf();
    }
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT".into());
    let dirs = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&dirs) {
        for ext in exts.split(';').filter(|e| !e.is_empty()) {
            let candidate = dir.join(format!("{name}{}", ext.to_ascii_lowercase()));
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split() {
        assert_eq!(
            split_args(r#"code --goto {file}:{line}"#),
            ["code", "--goto", "{file}:{line}"]
        );
        assert_eq!(
            split_args(r#""C:\Program Files\x.exe" -g "{file}" """#),
            [r"C:\Program Files\x.exe", "-g", "{file}", ""]
        );
    }
}
