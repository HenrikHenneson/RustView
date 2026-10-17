//! Analyses on `rv-ir` bodies. Independent of the frontend: a future
//! rust-analyzer backend feeds the same functions.

pub mod dataflow;

pub use dataflow::{Binding, DataFlow, Edge, EdgeKind, data_flow};
