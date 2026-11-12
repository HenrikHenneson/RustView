//! MCP server: `rustview mcp [project]` answers questions about the structure
//! of a Rust project over stdio — threads, channels, shared state, callers,
//! nesting — for AI coding assistants. The project is loaded once and
//! reloaded when one of its files changed.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router,
};
use rv_analysis::{CallGraph, Facts, Parts, Program, query};
use rv_ir::{FnId, Project};
use serde::Deserialize;

const INSTRUCTIONS: &str = "Structure of the Rust project in the working directory, from a \
syntactic analysis (macros not expanded, names resolved by heuristics): threads and who starts \
them, channels, shared state, call graph and the nesting of functions. Start with `report` (or \
`threads` for concurrency questions) to get the map of the project, then ask `find`, `callers`, \
`callees`, `threads_of` or `outline` about single names; `health` lists what is too long, too \nnested or tangled. Answers carry file:line references — \
read the code there before relying on a detail.";

/// Serves the project at `root` over stdin/stdout until the client closes it.
pub fn serve(root: PathBuf) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let server = RustViewServer::new(root)
            .serve(rmcp::transport::stdio())
            .await?;
        server.waiting().await?;
        Ok(())
    })
}

/// A loaded project with its analyses.
struct Loaded {
    project: Project,
    facts: Facts,
    calls: CallGraph,
    programs: Vec<(FnId, Program)>,
    /// Newest modification time of the project's files when loaded.
    stamp: Option<SystemTime>,
}

#[derive(Clone)]
pub struct RustViewServer {
    root: PathBuf,
    loaded: Arc<Mutex<Option<Loaded>>>,
    tool_router: ToolRouter<Self>,
}

/// Newest modification time of the loaded files and manifests: a change in
/// any of them (also a new `mod` line for a new file) means reloading.
fn stamp(project: &Project) -> Option<SystemTime> {
    project
        .files
        .iter()
        .map(|f| f.path.as_path())
        .chain(project.packages.iter().map(|p| p.manifest_path.as_path()))
        .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .max()
}

impl RustViewServer {
    pub fn new(root: PathBuf) -> Self {
        RustViewServer {
            root,
            loaded: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        }
    }

    /// Runs `f` on the current state of the project, (re)loading it first if needed.
    fn with<T>(&self, f: impl FnOnce(&Loaded) -> T) -> Result<T, String> {
        let mut guard = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        let fresh = guard.as_ref().is_some_and(|l| stamp(&l.project) == l.stamp);
        if !fresh {
            *guard = Some(load(&self.root)?);
        }
        Ok(f(guard.as_ref().expect("loaded above")))
    }

    /// Runs a question about one function.
    fn about_fn(
        &self,
        name: &str,
        f: impl FnOnce(&Loaded, FnId) -> String,
    ) -> Result<CallToolResult, McpError> {
        answer(self.with(|l| query::resolve_function(&l.project, name).map(|id| f(l, id))))
    }
}

fn load(root: &Path) -> Result<Loaded, String> {
    let project = rv_syn::load(root)
        .map_err(|e| format!("Could not load a Rust project at {}: {e:#}", root.display()))?;
    let facts = Facts::new(&project);
    let calls = CallGraph::new(&project);
    let programs = rv_analysis::programs(&project, &facts, &calls);
    let stamp = stamp(&project);
    Ok(Loaded {
        project,
        facts,
        calls,
        programs,
        stamp,
    })
}

/// Text answer; errors of the question (unknown name, ...) are tool errors
/// the model can read, not protocol errors.
fn answer(result: Result<Result<String, String>, String>) -> Result<CallToolResult, McpError> {
    Ok(match result.and_then(|r| r) {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(text) => CallToolResult::error(vec![ContentBlock::text(text)]),
    })
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NameArgs {
    /// A name: function (`ethercat_task`, `App::update`, `crate::module::f`),
    /// type, thread name, channel end or shared-state variable.
    pub name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FunctionArgs {
    /// Function by full path or any `::`-suffix: `update`, `App::update`,
    /// `my_crate::gui::App::update`. Ambiguous names list the candidates.
    pub function: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OutlineArgs {
    /// Function by full path or any `::`-suffix.
    pub function: String,
    /// Also list plain statements, not only loops, branches, closures,
    /// thread starts and exits. Default false.
    #[serde(default)]
    pub statements: bool,
}

#[tool_router]
impl RustViewServer {
    #[tool(
        description = "Markdown map of the whole project (a few thousand tokens): packages, \
        entry points, every thread with where it is started and what it runs, channels and \
        shared state with each send/receive/lock site and its effect (blocking, non-blocking, \
        may drop), module graph, largest and most called functions, names to search for. \
        Use it first when starting on unfamiliar code."
    )]
    async fn report(&self) -> Result<CallToolResult, McpError> {
        answer(self.with(|l| {
            Ok(rv_analysis::report_with(
                &l.project,
                &l.facts,
                &l.calls,
                &l.programs,
                Parts::ALL,
            ))
        }))
    }

    #[tool(
        description = "Only the concurrency part of the report: threads and tasks of every \
        binary, channels, shared state and joins, with file:line of every site. Use before \
        changing code that runs on several threads."
    )]
    async fn threads(&self) -> Result<CallToolResult, McpError> {
        answer(self.with(|l| {
            let parts = Parts {
                overview: false,
                threads: true,
                modules: false,
                hot_spots: false,
            };
            Ok(rv_analysis::report_with(
                &l.project,
                &l.facts,
                &l.calls,
                &l.programs,
                parts,
            ))
        }))
    }

    #[tool(
        description = "Code health of the project as Markdown: a 0-100 score and the functions         (length, branching complexity, nesting depth, parameters), files and module call cycles         over their targets and limits, worst first with file:line. Use it to decide what to         refactor, and after a change to check that no new finding appeared."
    )]
    async fn health(&self) -> Result<CallToolResult, McpError> {
        answer(self.with(|l| {
            let health = rv_analysis::Health::new(&l.project, &l.calls);
            Ok(rv_analysis::health_report(&l.project, &health))
        }))
    }

    #[tool(
        description = "What a name is: matching functions (with signature), a type's fields \
        and methods, type aliases, threads with that name, channels and shared state using it."
    )]
    async fn find(
        &self,
        Parameters(args): Parameters<NameArgs>,
    ) -> Result<CallToolResult, McpError> {
        answer(self.with(|l| Ok(query::find(&l.project, &l.programs, &args.name))))
    }

    #[tool(
        description = "Functions of the project that call the given function, with the call lines."
    )]
    async fn callers(
        &self,
        Parameters(args): Parameters<FunctionArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.about_fn(&args.function, |l, f| {
            query::callers(&l.project, &l.calls, f)
        })
    }

    #[tool(description = "Project functions the given function calls, with the call lines.")]
    async fn callees(
        &self,
        Parameters(args): Parameters<FunctionArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.about_fn(&args.function, |l, f| {
            query::callees(&l.project, &l.calls, f)
        })
    }

    #[tool(
        description = "Which threads run the given function (per binary), and whether it is \
        the thread's entry, reached through calls, or a framework callback."
    )]
    async fn threads_of(
        &self,
        Parameters(args): Parameters<FunctionArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.about_fn(&args.function, |l, f| {
            query::threads_of(&l.project, &l.programs, f)
        })
    }

    #[tool(
        description = "Nesting of a function as an indented outline with line ranges: loops, \
        if/match cases, closures, async blocks, thread starts (‖) and exits (→ return, ?, \
        break). Much shorter than the source; use it to navigate long functions before \
        reading the relevant lines."
    )]
    async fn outline(
        &self,
        Parameters(args): Parameters<OutlineArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.about_fn(&args.function, |l, f| {
            query::outline(&l.project, &l.facts, f, args.statements)
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RustViewServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("rustview", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}
