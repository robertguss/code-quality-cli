//! Tells production code from test and dev code by path alone, for checks
//! that only care about what ships.

/// What a file is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Production,
    Test,
    /// Benchmarks and examples: not shipped, but not tests either.
    Dev,
}

/// Directory names that hold tests or test data.
const TEST_DIRS: &[&str] = &["tests", "test", "__tests__", "spec", "testdata"];
/// Directory names that hold benchmarks and examples.
const DEV_DIRS: &[&str] = &["benches", "examples"];

/// The role of the file at `path`, from its directory names and file name.
/// A directory counts by its name only, so `lib/contest/` is production.
pub fn classify(path: &str) -> Role {
    let mut segments: Vec<&str> = path.split('/').collect();
    let name = segments.pop().unwrap_or_default();
    if segments.iter().any(|s| TEST_DIRS.contains(s)) || is_test_name(name) {
        return Role::Test;
    }
    if segments.iter().any(|s| DEV_DIRS.contains(s)) {
        return Role::Dev;
    }
    Role::Production
}

fn is_test_name(name: &str) -> bool {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    name == "conftest.py"
        || name.ends_with("_test.go")
        || name.ends_with("_test.exs")
        || (name.ends_with(".py") && (stem.starts_with("test_") || stem.ends_with("_test")))
        || stem.ends_with(".test")
        || stem.ends_with(".spec")
}
