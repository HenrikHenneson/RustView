//! Data flow and concurrency facts of every function of a project.

use rv_ir::{FnId, Project};

use crate::{Concurrency, DataFlow};

/// Data flow and concurrency facts of every function.
pub struct Facts {
    flow: Vec<Option<DataFlow>>,
    conc: Vec<Option<Concurrency>>,
}

impl Facts {
    pub fn new(project: &Project) -> Self {
        let mut flow = Vec::with_capacity(project.functions.len());
        let mut conc = Vec::with_capacity(project.functions.len());
        for f in &project.functions {
            match &f.body {
                Some(body) => {
                    let fl = crate::data_flow(body);
                    conc.push(Some(crate::concurrency(body, &fl, &project.type_aliases)));
                    flow.push(Some(fl));
                }
                None => {
                    flow.push(None);
                    conc.push(None);
                }
            }
        }
        Facts { flow, conc }
    }

    pub fn flow(&self, f: FnId) -> Option<&DataFlow> {
        self.flow[f.index()].as_ref()
    }

    pub fn concurrency(&self, f: FnId) -> Option<&Concurrency> {
        self.conc[f.index()].as_ref()
    }
}
