//! Slice 1 rules through the installed hooks (U12).

use qgate::date::Date;

use super::support::repo::TestRepo;
use super::{func, git, on_feature_branch, onboard};

fn commit(t: &TestRepo, msg: &str) -> (bool, String) {
    git(t, &["commit", "-q", "-m", msg])
}

#[test]
fn cast_and_console_log_are_refused_until_fixed() {
    // F1
    let t = TestRepo::new();
    t.write("src/user.ts", "export interface User { name: string }\n");
    onboard(&t);
    t.write(
        "src/show.ts",
        "import { User } from './user';\nexport function show(data: unknown) {\n  const user = data as User;\n  console.log(user);\n}\n",
    );
    t.git(&["add", "src/show.ts"]);
    let (ok, out) = commit(&t, "show");
    assert!(!ok, "{out}");
    assert!(
        out.contains("src/show.ts:3: error[escape-hatch/as-cast]"),
        "{out}"
    );
    assert!(
        out.contains("src/show.ts:4: error[debug-output/console-log]"),
        "{out}"
    );

    t.write(
        "src/show.ts",
        "import { User } from './user';\nexport function show(user: User): string {\n  return user.name;\n}\n",
    );
    t.git(&["add", "src/show.ts"]);
    let (ok, out) = commit(&t, "show");
    assert!(ok, "{out}");
}

#[test]
fn upgrading_the_pin_records_new_rules_as_one_warning() {
    let t = TestRepo::new();
    t.write(
        "src/a.rs",
        "pub fn f(v: Option<u8>) -> u8 {\n    v.unwrap()\n}\n",
    );
    t.write("src/big.rs", &func("big", 70));
    t.write("qgate.toml", "qgate_version = \"0.4.1\"\n");
    t.write("qgate.baseline", "function-length\tsrc/big.rs\tbig#0\t70\n");
    t.commit_all("gated by 0.4.1");
    t.qgate().arg("install").assert().success();
    t.write(
        "qgate.toml",
        &format!("qgate_version = \"{}\"\n", qgate::config::VERSION),
    );
    t.qgate()
        .args(["baseline", "create", "--force"])
        .assert()
        .success();
    assert!(
        t.read("qgate.baseline")
            .contains("escape-hatch/rust-unwrap\tsrc/a.rs\t-\t1")
    );
    t.git(&["add", "-A"]);
    let (ok, out) = commit(&t, "upgrade qgate");
    assert!(ok, "{out}");
    assert!(out.contains("upgrade to"), "{out}");
}

#[test]
fn new_suppression_needs_a_reason_and_expiry() {
    let t = TestRepo::new();
    t.write("src/a.ts", "export const a = 1;\n");
    onboard(&t);
    let until = Date::today().add_days(30);
    t.write(
        "src/a.ts",
        "// eslint-disable-next-line no-console\nexport const a = 1;\n",
    );
    t.git(&["add", "src/a.ts"]);
    let (ok, out) = commit(&t, "bare");
    assert!(!ok, "{out}");
    assert!(out.contains("error[suppression]"), "{out}");

    t.write(
        "src/a.ts",
        &format!("// eslint-disable-next-line no-console -- reason: CLI output, until: {until}\nexport const a = 1;\n"),
    );
    t.git(&["add", "src/a.ts"]);
    let (ok, out) = commit(&t, "explained");
    assert!(ok, "{out}");
    assert!(out.contains("warning[loosening]"), "{out}");

    t.write(
        "src/b.ts",
        &format!("/* eslint-disable -- reason: legacy, until: {until} */\nexport const b = 1;\n"),
    );
    t.git(&["add", "src/b.ts"]);
    let (ok, out) = commit(&t, "blanket");
    assert!(!ok, "{out}");
    assert!(out.contains("always_block"), "{out}");
}

#[test]
fn push_checks_the_tip_so_a_removed_console_log_passes() {
    let (t, _remote) = on_feature_branch("src/a.ts", "export const a = 1;\n");
    t.write("src/a.ts", "export const a = 1;\nconsole.log(a);\n");
    t.git(&["add", "src/a.ts"]);
    t.git(&["commit", "-q", "--no-verify", "-m", "debug"]);
    t.write("src/a.ts", "export const a = 2;\n");
    t.git(&["add", "src/a.ts"]);
    let (ok, out) = commit(&t, "undebug");
    assert!(ok, "{out}");
    let (ok, out) = git(&t, &["push", "-q", "origin", "feature"]);
    assert!(ok, "{out}");
}
