//! Domain model of volee.
//!
//! This crate holds the types and rules shared by every part of volee:
//! agents and their sessions, roles and states, messages and threads,
//! missions and delivery evidence. It performs no IO, so everything in it is
//! tested without a terminal, a database, git or a coding harness.
//!
//! See the [architecture overview](https://volee-org.github.io/volee/architecture/overview.html)
//! for how the crates fit together.

/// The version of volee, as declared in the workspace manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_semver() {
        let parts: Vec<&str> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "unexpected version {VERSION}");
        assert!(parts.iter().all(|p| p.parse::<u64>().is_ok()));
    }
}
