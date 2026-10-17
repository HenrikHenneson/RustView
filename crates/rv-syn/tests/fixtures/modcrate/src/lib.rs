//! Fixture covering the module resolution rules.
pub mod flat;
mod dir;
#[path = "elsewhere/custom.rs"]
mod renamed;
mod inline_mod {
    pub fn in_inline() {}
    mod deeper;
}
mod missing;

pub fn free_fn(a: &mut Vec<u8>, b: i32) -> usize {
    fn helper() {}
    helper();
    a.len() + b as usize
}

pub struct S<T>(T);

impl<T> S<T> {
    pub async fn method(&self) {}
}

impl<T> std::fmt::Display for S<T> {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

pub trait Tr {
    fn provided(&self) {}
    fn required(&self);
}

#[cfg(not(test))]
pub fn not_test() {}

#[cfg(test)]
mod tests {
    #[test]
    fn a() {}
    #[test]
    fn b() {}
}
