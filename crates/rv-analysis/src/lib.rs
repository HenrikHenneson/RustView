//! Analyses on `rv-ir` bodies. Independent of the frontend: a future
//! rust-analyzer backend feeds the same functions.

pub mod concurrency;
pub mod dataflow;
pub mod modules;
pub mod program;
pub mod report;

pub use concurrency::{
    Channel, ChannelUse, Concurrency, End, SharedGroup, SharedUse, Spawn, SpawnKind, concurrency,
};
pub use dataflow::{Binding, DataFlow, Edge, EdgeKind, data_flow};
pub use modules::{ModuleEdge, ModuleGraph, ModuleNode, crates, module_graph};
pub use program::{
    CallGraph, ChannelComm, Facts, JoinComm, Link, LinkKind, Program, Resource, SharedComm, Site,
    Thread, entry_points, op_effect, op_meaning, op_writes, program,
};
pub use report::report;
