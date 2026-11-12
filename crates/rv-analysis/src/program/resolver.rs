//! Resolves call sites to project functions, by path (module/crate aware)
//! and for methods by `self`, the receiver's type or a unique name.

use std::collections::{HashMap, HashSet};

use rv_ir::{CallKind, CallSite, FnId, FnOwner, Package, Project, TargetKind};

/// Resolves call sites to project functions.
pub(super) struct Resolver<'p> {
    project: &'p Project,
    by_name: HashMap<&'p str, Vec<FnId>>,
    /// Crate names of library targets: callable from other crates. Other
    /// crates (binaries) only call into themselves.
    libs: HashSet<String>,
    /// Crate name of a target -> its package.
    package_of: HashMap<String, &'p Package>,
}

impl<'p> Resolver<'p> {
    pub(super) fn new(project: &'p Project) -> Self {
        let mut by_name: HashMap<&str, Vec<FnId>> = HashMap::new();
        for (i, f) in project.functions.iter().enumerate() {
            by_name
                .entry(f.name.as_str())
                .or_default()
                .push(FnId(i as u32));
        }
        let libs = project
            .packages
            .iter()
            .flat_map(|p| &p.targets)
            .filter(|t| t.kind == TargetKind::Lib)
            .map(|t| t.name.replace('-', "_"))
            .collect();
        let package_of = project
            .packages
            .iter()
            .flat_map(|p| p.targets.iter().map(move |t| (t.name.replace('-', "_"), p)))
            .collect();
        Resolver {
            project,
            by_name,
            libs,
            package_of,
        }
    }

    /// Whether `caller` can call `callee` at all: same crate, or a library
    /// the caller's package depends on (or its own library).
    fn visible(&self, caller: FnId, callee: FnId) -> bool {
        let to = self.crate_of(callee);
        let from = self.crate_of(caller);
        if to == from {
            return true;
        }
        if !self.libs.contains(to) {
            return false;
        }
        match (self.package_of.get(from), self.package_of.get(to)) {
            (Some(a), Some(b)) => std::ptr::eq(*a, *b) || a.dependencies.iter().any(|d| d == to),
            _ => true,
        }
    }

    fn crate_of(&self, f: FnId) -> &str {
        crate_name(&self.project.function(f).qual_path)
    }

    pub(super) fn resolve(&self, caller: FnId, call: &CallSite) -> Option<FnId> {
        let cands: Vec<FnId> = self
            .by_name
            .get(call.name())?
            .iter()
            .copied()
            .filter(|&c| self.visible(caller, c))
            .collect();
        match call.kind {
            CallKind::Macro => None,
            CallKind::Function => self.resolve_function(caller, call, &cands),
            CallKind::Method => self.resolve_method(caller, call, &cands),
        }
    }

    /// A call by path: `f()`, `module::f()`, `Type::f()`, `Self::f()`.
    fn resolve_function(&self, caller: FnId, call: &CallSite, cands: &[FnId]) -> Option<FnId> {
        let mut path: Vec<&str> = call.path.iter().map(String::as_str).collect();
        while matches!(path.first(), Some(&"crate" | &"self" | &"super")) {
            path.remove(0);
        }
        let self_ty = self_type(&self.project.function(caller).owner);
        if path.first() == Some(&"Self") {
            path[0] = self_ty?;
        }
        if path.len() == 1 {
            self.resolve_plain(caller, cands)
        } else {
            self.resolve_qualified(caller, &path.join("::"), cands)
        }
    }

    /// `f()`: a function nested in the caller, a free function of the same
    /// module, or the only free function of that name in the crate.
    fn resolve_plain(&self, caller: FnId, cands: &[FnId]) -> Option<FnId> {
        let p = self.project;
        let caller_fn = p.function(caller);
        let nested = cands.iter().find(
            |c| matches!(p.function(**c).owner, FnOwner::Nested { parent } if parent == caller),
        );
        let same_module = cands.iter().find(|c| {
            let f = p.function(**c);
            f.module == caller_fn.module && f.owner == FnOwner::Free
        });
        let free_in_crate: Vec<&FnId> = cands
            .iter()
            .filter(|c| p.function(**c).owner == FnOwner::Free)
            .filter(|c| self.crate_of(**c) == self.crate_of(caller))
            .collect();
        nested
            .or(same_module)
            .or((free_in_crate.len() == 1).then(|| free_in_crate[0]))
            .copied()
    }

    /// `a::b::f()`: the function whose qualified path ends with `full`,
    /// preferring the caller's crate.
    fn resolve_qualified(&self, caller: FnId, full: &str, cands: &[FnId]) -> Option<FnId> {
        let suffix = format!("::{full}");
        let matching: Vec<&FnId> = cands
            .iter()
            .filter(|c| {
                let q = &self.project.function(**c).qual_path;
                // `mtd::x::f` from a binary calling into its library.
                *q == full || q.ends_with(&suffix)
            })
            .collect();
        matching
            .iter()
            .find(|c| self.crate_of(***c) == self.crate_of(caller))
            .or(matching.first())
            .map(|c| **c)
    }

    /// `x.f()`: by `self`, by the receiver's type, by a type named in the
    /// chain, or the only project method of that name.
    fn resolve_method(&self, caller: FnId, call: &CallSite, cands: &[FnId]) -> Option<FnId> {
        let p = self.project;
        let owner_type = |c: FnId| self_type(&p.function(c).owner);
        let methods: Vec<FnId> = cands
            .iter()
            .copied()
            .filter(|&c| {
                matches!(
                    p.function(c).owner,
                    FnOwner::Impl { .. } | FnOwner::Trait { .. }
                )
            })
            .collect();
        let own = self_type(&p.function(caller).owner);
        if call.receiver.as_deref() == Some("self") && call.field.is_none() {
            return methods
                .iter()
                .copied()
                .find(|&c| own.is_some() && owner_type(c) == own);
        }
        // Type of the receiver: a field's declared type or a variable's annotation.
        let hints = self.receiver_types(caller, call);
        let by_hint: Vec<FnId> = methods
            .iter()
            .copied()
            .filter(|&c| owner_type(c).is_some_and(|t| hints.iter().any(|h| h == t)))
            .collect();
        if by_hint.len() == 1 {
            return Some(by_hint[0]);
        }
        // A type named in the chain (`Faden::neu(..).laufen()`).
        let typed: Vec<FnId> = methods
            .iter()
            .copied()
            .filter(|&c| owner_type(c).is_some_and(|t| call.path.iter().any(|s| s == t)))
            .collect();
        if typed.len() == 1 {
            return Some(typed[0]);
        }
        // The only project method of that name — unless the name is so
        // common on std types that the receiver is more likely a `Vec`,
        // `str`, `Option`, ... than the project type.
        (methods.len() == 1 && !STD_METHODS.contains(&call.name())).then(|| methods[0])
    }

    /// Type names that may be the receiver's type: from the declared type of
    /// `self.field` or of the receiver variable (`x: &Arc<Geteilt>`).
    fn receiver_types(&self, caller: FnId, call: &CallSite) -> Vec<String> {
        let p = self.project;
        let func = p.function(caller);
        let receiver = call.receiver.as_deref();
        let declared = match receiver {
            Some("self") => self_type(&func.owner).map(str::to_string),
            Some(r) => func.body.as_ref().and_then(|b| {
                b.nodes
                    .iter()
                    .flat_map(|n| &n.defs)
                    .find(|d| d.name == r)
                    .and_then(|d| d.ty.clone())
            }),
            None => None,
        };
        let Some(declared) = declared else {
            return Vec::new();
        };
        let ty = match &call.field {
            // `self.handregler.x()` / `geteilt.stand.lock()`: type of the field.
            Some(field) => type_names(&declared).iter().find_map(|s| {
                p.struct_fields
                    .get(s)?
                    .iter()
                    .find(|(f, _)| f == field)
                    .map(|(_, t)| t.clone())
            }),
            None => Some(declared),
        };
        ty.map(|t| type_names(&t)).unwrap_or_default()
    }
}

/// The crate of a qualified path: `app` for `app::gui::run`.
pub(super) fn crate_name(qual_path: &str) -> &str {
    qual_path.split("::").next().unwrap_or("")
}

/// Capitalized identifiers in a type (`Arc<Mutex<Geteilt>>` -> Arc, Mutex, Geteilt).
fn type_names(ty: &str) -> Vec<String> {
    ty.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| w.chars().next().is_some_and(char::is_uppercase))
        .map(str::to_string)
        .collect()
}

/// `App` for methods of `impl App` / `impl<T> Trait for App<T>`.
pub(super) fn self_type(owner: &FnOwner) -> Option<&str> {
    match owner {
        FnOwner::Impl { self_ty, .. } => {
            let base = self_ty.split('<').next().unwrap_or(self_ty).trim();
            Some(base.rsplit("::").next().unwrap_or(base))
        }
        _ => None,
    }
}

/// Method names of common std types (collections, strings, Option/Result,
/// iterators, smart pointers). A call like `x.len()` without a known receiver
/// type is not guessed to be the project's only `len`.
const STD_METHODS: &[&str] = &[
    "len",
    "is_empty",
    "iter",
    "iter_mut",
    "into_iter",
    "get",
    "get_mut",
    "insert",
    "remove",
    "push",
    "pop",
    "clear",
    "contains",
    "contains_key",
    "extend",
    "first",
    "last",
    "as_str",
    "as_ref",
    "as_mut",
    "as_slice",
    "to_string",
    "to_owned",
    "clone",
    "map",
    "unwrap",
    "unwrap_or",
    "unwrap_or_default",
    "ok",
    "err",
    "is_some",
    "is_none",
    "take",
    "replace",
    "min",
    "max",
    "sum",
    "count",
    "join",
    "split",
    "trim",
    "parse",
    "lines",
    "chars",
    "bytes",
    "keys",
    "values",
    "entry",
    "sort",
    "dedup",
    "retain",
    "drain",
    "truncate",
    "reset",
    "next",
    "label",
    "name",
    "kind",
    "value",
    "start",
    "end",
    "set",
    "add",
    "sub",
    "abs",
    "round",
    "from",
    "into",
    "default",
    "eq",
    "cmp",
    "hash",
    "fmt",
    "read",
    "write",
    "flush",
    "close",
];
