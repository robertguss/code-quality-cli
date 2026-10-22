//! `qgate features init` and the drafted map in `qgate install` (R7).

mod support;

use support::gate::{commit, findings};
use support::repo::TestRepo;

/// A committed crate with a binary, a folder module, a file module and a `src/bin` binary.
fn crate_repo() -> TestRepo {
    let t = TestRepo::new();
    t.write("qgate.toml", &qgate::config::Config::starter().to_toml());
    t.write(
        "Cargo.toml",
        "[package]\nname = \"shop\"\nversion = \"0.1.0\"\n",
    );
    t.write("src/main.rs", "mod baseline;\nmod hash;\nfn main() {}\n");
    t.write("src/baseline/mod.rs", "\n");
    t.write("src/hash.rs", "\n");
    t.write("src/bin/import.rs", "fn main() {}\n");
    t.write("migrations/V1__a.sql", "create table a (id int);\n");
    t.commit_all("start");
    t
}

fn init(t: &TestRepo, args: &[&str]) -> std::process::Output {
    t.qgate()
        .args(["features", "init"])
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn init_drafts_one_placeholder_per_module_and_groups_the_rest() {
    let t = crate_repo();
    let out = init(&t, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let map = qgate::features::map::FeatureMap::parse(&t.read("qgate.features.toml")).unwrap();
    let names: Vec<&str> = map.features.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["baseline", "hash", "migrations", "src", "src-bin"]);
    let get = |n: &str| map.get(n).unwrap();
    assert!(map.features.iter().all(|f| f.placeholder));
    assert_eq!(get("baseline").folders, ["src/baseline"]);
    assert_eq!(get("hash").entry_points, ["src/hash.rs"]);
    assert_eq!(get("src").entry_points, ["src/main.rs"]);
    assert_eq!(get("src-bin").entry_points, ["src/bin/import.rs"]);
    assert!(get("src-bin").folders.is_empty());
    assert_eq!(get("migrations").folders, ["migrations"]);
}

#[test]
fn the_draft_passes_a_commit_with_one_placeholder_line() {
    let t = crate_repo();
    assert!(init(&t, &[]).status.success());
    let (passed, report) = commit(&t);
    assert!(passed, "{report}");
    assert_eq!(findings(&report, "feature-placeholder").len(), 1);
    assert!(findings(&report, "feature-unowned").is_empty());
    t.commit_all("map");
    // A routine migration in the owned folder needs no map edit (AE6).
    t.write("migrations/V2__b.sql", "create table b (id int);\n");
    let (passed, report) = commit(&t);
    assert!(passed, "{report}");
    assert!(findings(&report, "scope-change").is_empty());
}

#[test]
fn the_draft_is_the_same_every_time() {
    let t = crate_repo();
    let a = init(&t, &["--dry-run", "--format", "json"]);
    let b = init(&t, &["--dry-run", "--format", "json"]);
    let map = |o: &std::process::Output| {
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        v["data"]["map"].as_str().unwrap().to_string()
    };
    assert_eq!(map(&a), map(&b));
}

#[test]
fn init_refuses_to_replace_a_map_without_force() {
    let t = crate_repo();
    t.write("qgate.features.toml", "# mine\n");
    let out = init(&t, &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--force"));
    assert_eq!(t.read("qgate.features.toml"), "# mine\n");
    let out = init(&t, &["--dry-run"]);
    assert!(out.status.success());
    assert_eq!(t.read("qgate.features.toml"), "# mine\n");
    assert!(init(&t, &["--force"]).status.success());
    assert!(t.read("qgate.features.toml").contains("[[feature]]"));
}

#[test]
fn install_writes_the_draft_once() {
    let t = crate_repo();
    let out = t.qgate().arg("install").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        t.read("qgate.features.toml")
            .contains("name = \"baseline\"")
    );
    t.write("qgate.features.toml", "# edited\n");
    assert!(t.qgate().arg("install").output().unwrap().status.success());
    assert_eq!(t.read("qgate.features.toml"), "# edited\n");
}

#[test]
fn install_drafts_without_a_qgate_toml() {
    let t = TestRepo::new();
    t.write(
        "Cargo.toml",
        "[package]\nname = \"shop\"\nversion = \"0.1.0\"\n",
    );
    t.write("src/lib.rs", "pub mod core;\n");
    t.write("src/core.rs", "\n");
    t.commit_all("start");
    let out = t.qgate().arg("install").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(t.read("qgate.features.toml").contains("name = \"core\""));
}

#[test]
fn init_works_without_a_qgate_toml() {
    let t = crate_repo();
    t.remove("qgate.toml");
    t.commit_all("no config");
    assert!(init(&t, &[]).status.success());
    assert!(
        t.read("qgate.features.toml")
            .contains("name = \"baseline\"")
    );
}

#[test]
fn a_module_file_and_its_folder_are_one_placeholder() {
    let t = crate_repo();
    t.write("src/hash/sha.rs", "\n");
    t.commit_all("hash folder");
    assert!(init(&t, &[]).status.success());
    let map = qgate::features::map::FeatureMap::parse(&t.read("qgate.features.toml")).unwrap();
    let hash = map.get("hash").unwrap();
    assert_eq!(hash.folders, ["src/hash"]);
    assert_eq!(hash.entry_points, ["src/hash.rs"]);
    assert_eq!(map.features.len(), 5);
}
