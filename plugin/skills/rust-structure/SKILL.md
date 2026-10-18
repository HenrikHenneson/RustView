---
name: rust-structure
description: Map the structure of a Rust project with the RustView MCP tools — threads and who starts them, channels, shared state (Arc/Mutex/atomics), callers and callees, and the nesting of long functions, all with file:line. Use before changing concurrent code, when getting oriented in an unfamiliar or large Rust codebase, or when asked which thread runs something, who sends on a channel, who uses a shared object, or where a function is called.
when_to_use: Rust project with Cargo.toml in the working directory; questions like "how do the threads communicate", "who calls X", "which thread runs Y", "give me an overview of this crate", "where does this loop end".
---

# Rust project structure with RustView

The `rustview` MCP server analyzes the Rust project in the working directory
(syntactically: macros are not expanded, names are resolved by heuristics).
It keeps the project loaded and reloads it when files change, so answers are
current and fast.

## Which tool when

| Question | Tool |
|---|---|
| First look at an unfamiliar project | `report` (a few thousand tokens: threads, channels, shared state, modules, hot spots, names to search for) |
| Anything about concurrency | `threads` (only the concurrency part of the report) |
| What is `X`? (function, type, thread, channel end, shared variable) | `find` with `name` |
| Who calls `f` / what does `f` call | `callers` / `callees` with `function` |
| On which thread does `f` run | `threads_of` with `function` |
| Shape of a long function before reading it | `outline` with `function` (add `statements: true` for every statement) |

Function names can be given as full path or any `::`-suffix (`ethercat_task`,
`App::update`). An ambiguous name returns the candidates — repeat with a
longer path.

## How to use the answers

- Every answer has `file:line` references. Read the code there (a few lines
  around them) before relying on a detail; the analysis is a map, not proof.
- Prefer `outline` + reading the relevant line range over reading a whole
  long function.
- Effects in the thread sections (`blocking`, `non-blocking`, `may drop`,
  `atomic`) come from the API method and channel capacity, not from the
  surrounding code. `may drop` (`try_send`) deserves a look at how the error
  is handled.
- `?` / `not followed` marks a value the analysis lost track of; search for
  it in the code if it matters.
- After changing threads, channels or shared state, call `threads` again and
  compare with the state before, to catch unintended structural changes.
