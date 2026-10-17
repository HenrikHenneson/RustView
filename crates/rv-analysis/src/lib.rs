//! Analyses on `rv-ir` bodies. Independent of the frontend: a future
//! rust-analyzer backend feeds the same functions.

pub mod concurrency;
pub mod dataflow;
pub mod program;

pub use concurrency::{
    Channel, ChannelUse, Concurrency, End, SharedGroup, SharedUse, Spawn, SpawnKind, concurrency,
};
pub use dataflow::{Binding, DataFlow, Edge, EdgeKind, data_flow};
pub use program::{
    CallGraph, Facts, Link, LinkKind, Program, Resource, Thread, entry_points, program,
};
