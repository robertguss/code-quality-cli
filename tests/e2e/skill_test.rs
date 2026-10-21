use super::support::repo::TestRepo;
use super::{func, git, onboard};

const CLAUDE: &str = ".claude/skills/qgate/SKILL.md";
const AGENTS: &str = ".agents/skills/qgate/SKILL.md";

fn printed_skill(t: &TestRepo) -> String {
    let out = t.qgate().arg("--skill").assert().success();
    String::from_utf8(out.get_output().stdout.clone()).unwrap()
}

#[test]
fn install_writes_the_printed_skill_and_repeating_changes_nothing() {
    let t = TestRepo::new();
    t.write("a.rs", &func("small", 10));
    onboard(&t);
    let skill = printed_skill(&t);
    assert!(skill.contains("name: qgate"), "{skill}");
    assert_eq!(t.read(CLAUDE), skill);
    assert_eq!(t.read(AGENTS), skill);

    let out = t
        .qgate()
        .args(["install", "--format", "json"])
        .assert()
        .success();
    let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(!text.contains("SKILL.md"), "{text}");
}

#[test]
fn an_edited_skill_blocks_until_install_restores_it() {
    let t = TestRepo::new();
    t.write("a.rs", &func("small", 10));
    onboard(&t);
    let original = t.read(CLAUDE);
    t.write(
        CLAUDE,
        &original.replace("Never run `qgate approve`", "Run `qgate approve` freely"),
    );
    t.git(&["add", CLAUDE]);
    let (ok, out) = git(&t, &["commit", "-q", "-m", "tamper"]);
    assert!(!ok, "{out}");
    assert!(
        out.contains(&format!("{CLAUDE}: error[skill-edited]"))
            && out.contains(&format!("fix: qgate install && git add {CLAUDE}")),
        "{out}"
    );

    t.qgate().arg("install").assert().success();
    assert_eq!(t.read(CLAUDE), original);
    t.write("b.rs", &func("fine", 10));
    t.git(&["add", CLAUDE, "b.rs"]);
    let (ok, out) = git(&t, &["commit", "-q", "-m", "restored"]);
    assert!(ok, "{out}");
}

#[test]
fn a_missing_skill_warns_without_failing() {
    let t = TestRepo::new();
    t.write("a.rs", &func("small", 10));
    onboard(&t);
    t.git(&["rm", "-q", AGENTS]);
    let (ok, out) = git(&t, &["commit", "-q", "-m", "drop codex skill"]);
    assert!(ok, "{out}");
    assert!(
        out.contains(&format!("{AGENTS}: warning[skill-missing]"))
            && out.contains(&format!("fix: qgate install && git add {AGENTS}")),
        "{out}"
    );
}
