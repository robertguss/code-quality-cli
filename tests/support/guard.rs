//! Run the loosening guard on a test repo's staged change.

use qgate::config::VERSION;
use qgate::date::Date;
use qgate::git::{Snapshot, changes};
use qgate::guard::{self, Loosening};

use super::repo::TestRepo;

/// The loosening the staged change would introduce.
pub fn staged(t: &TestRepo) -> Loosening {
    let repo = t.repo();
    let parent = Snapshot::at(&repo, repo.head().as_deref()).unwrap();
    let current = Snapshot::index(&repo).unwrap();
    let changes = changes::staged(&repo).unwrap();
    guard::loosening(&repo, &parent, &current, &changes, today()).unwrap()
}

pub fn today() -> Date {
    Date::parse("2026-10-22").unwrap()
}

pub fn pending(t: &TestRepo) -> Vec<String> {
    staged(t).lines.into_iter().map(|(_, l)| l).collect()
}

/// Pending lines match `expected` up to the content digest each line ends with.
#[track_caller]
pub fn assert_pending(t: &TestRepo, expected: &[&str]) {
    let lines = pending(t);
    assert_eq!(lines.len(), expected.len(), "{lines:?}");
    for (line, want) in lines.iter().zip(expected) {
        assert!(
            line.starts_with(want),
            "{line:?} should start with {want:?}"
        );
    }
}

pub fn warned(t: &TestRepo) -> bool {
    !guard::check(&staged(t)).is_empty()
}

/// A test config with the given function limit, an eslint tool and a push tool.
pub fn config(function_lines: u32) -> String {
    format!(
        "qgate_version = \"{VERSION}\"\nexclude = [\"testdata/**\"]\n\n[limits]\nfunction_lines = {function_lines}\n\n[[tool]]\nname = \"eslint\"\ncommand = \"npx eslint {{files}}\"\nglobs = [\"**/*.js\"]\nconfig_files = [\".eslintrc.json\"]\n\n[[tool]]\nname = \"tests\"\ncommand = \"npm test\"\nstage = \"push\"\n"
    )
}

/// A committed repo with config, an eslint config and package scripts.
pub fn setup() -> TestRepo {
    let t = TestRepo::new();
    t.write("qgate.toml", &config(50))
        .write(".eslintrc.json", "{\"rules\": {\"no-var\": \"error\"}}\n")
        .write(
            "package.json",
            "{\"name\": \"x\", \"version\": \"1.0.0\", \"scripts\": {\"test\": \"jest\"}}\n",
        )
        .write(
            "src/app.js",
            "const a = 1;\n// eslint-disable-next-line no-console\nconsole.log(a);\nconst b = 2;\n",
        );
    t.commit_all("init");
    t
}
