//! A repo gated by qgate with a feature map, and helpers to run it.

use super::repo::TestRepo;

pub const CARGO: &str = "[package]\nname = \"shop\"\nversion = \"0.1.0\"\n";

pub const MAP: &str = "[[feature]]\nname = \"core\"\npurpose = \"The shop's core\"\ntask = \"QG-1\"\nfolders = [\"src/core\"]\n";

/// A committed crate whose one module, `src/core`, is owned by feature `core`.
pub fn gated() -> TestRepo {
    let t = TestRepo::new();
    t.write("qgate.toml", &qgate::config::Config::starter().to_toml());
    t.write("Cargo.toml", CARGO);
    t.write("src/lib.rs", "pub mod core;\n");
    t.write("src/core/mod.rs", "pub fn price() -> u32 {\n    1\n}\n");
    t.write("qgate.features.toml", MAP);
    t.commit_all("start");
    t
}

/// Append a feature to the map in the working tree.
pub fn add_feature(t: &TestRepo, toml: &str) {
    let map = t.read("qgate.features.toml");
    t.write("qgate.features.toml", &format!("{map}\n{toml}"));
}

/// Run qgate with JSON output: whether it passed, and the report.
pub fn run(t: &TestRepo, args: &[&str]) -> (bool, serde_json::Value) {
    let out = t
        .qgate()
        .args(args)
        .args(["--format", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("{e}: {stdout}{}", String::from_utf8_lossy(&out.stderr)));
    (out.status.success(), report)
}

pub fn commit(t: &TestRepo) -> (bool, serde_json::Value) {
    t.git(&["add", "-A"]);
    run(t, &["run", "--stage", "commit"])
}

/// `rule: message` for each finding whose rule starts with `prefix`.
pub fn findings(report: &serde_json::Value, prefix: &str) -> Vec<String> {
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["rule"].as_str().unwrap().starts_with(prefix))
        .map(|f| {
            format!(
                "{}: {}",
                f["rule"].as_str().unwrap(),
                f["message"].as_str().unwrap()
            )
        })
        .collect()
}
