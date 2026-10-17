//! Syntax highlighting with syntect on a worker thread.
//!
//! Highlighting a 10k-line file takes a noticeable moment, so the code panel
//! shows plain text until the worker is done.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::Color32;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Styled byte ranges of one line, relative to the line start.
pub type LineStyles = Vec<(Range<usize>, Color32, bool)>;

pub struct Highlighted {
    pub lines: Vec<LineStyles>,
}

/// Identifies one version of a file's text in one theme.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    text: *const u8,
    len: usize,
    dark: bool,
}

// The pointer is only used as an identity, never dereferenced.
unsafe impl Send for Key {}

struct Job {
    key: Key,
    text: Arc<str>,
}

pub struct Highlighter {
    jobs: Sender<Job>,
    done: Receiver<(Key, Highlighted)>,
    cache: HashMap<Key, Arc<Highlighted>>,
    pending: HashSet<Key>,
}

impl Highlighter {
    pub fn new(ctx: egui::Context) -> Self {
        let (jobs, job_rx) = channel::<Job>();
        let (done_tx, done) = channel();
        std::thread::Builder::new()
            .name("rustview-highlight".into())
            .spawn(move || {
                let syntaxes = SyntaxSet::load_defaults_newlines();
                let themes = ThemeSet::load_defaults();
                for job in job_rx {
                    let theme = if job.key.dark {
                        &themes.themes["base16-ocean.dark"]
                    } else {
                        &themes.themes["InspiredGitHub"]
                    };
                    let result = highlight(&syntaxes, theme, &job.text);
                    if done_tx.send((job.key, result)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn highlighter thread");
        Highlighter {
            jobs,
            done,
            cache: HashMap::new(),
            pending: HashSet::new(),
        }
    }

    /// Highlighting for `text`, or `None` while it is being computed.
    pub fn get(&mut self, text: &Arc<str>, dark: bool) -> Option<Arc<Highlighted>> {
        for (key, h) in self.done.try_iter() {
            self.pending.remove(&key);
            self.cache.insert(key, Arc::new(h));
        }
        let key = Key {
            text: text.as_ptr(),
            len: text.len(),
            dark,
        };
        if let Some(h) = self.cache.get(&key) {
            return Some(h.clone());
        }
        if self.pending.insert(key) {
            let _ = self.jobs.send(Job {
                key,
                text: text.clone(),
            });
        }
        None
    }

    /// Forget all results, e.g. after a reload replaced every text.
    pub fn clear(&mut self) {
        self.cache.clear();
    }
}

fn highlight(
    syntaxes: &SyntaxSet,
    theme: &syntect::highlighting::Theme,
    text: &str,
) -> Highlighted {
    let syntax = syntaxes
        .find_syntax_by_extension("rs")
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
    let mut h = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();
    for line in LinesWithEndings::from(text) {
        let mut styles = LineStyles::new();
        if let Ok(ranges) = h.highlight_line(line, syntaxes) {
            let mut pos = 0;
            for (style, piece) in ranges {
                let c = style.foreground;
                let italic = style.font_style.contains(FontStyle::ITALIC);
                styles.push((
                    pos..pos + piece.len(),
                    Color32::from_rgb(c.r, c.g, c.b),
                    italic,
                ));
                pos += piece.len();
            }
        }
        lines.push(styles);
    }
    Highlighted { lines }
}
