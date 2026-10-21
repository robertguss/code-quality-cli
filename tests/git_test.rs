mod support;

use qgate::git::{self, BlobReader, ChangeKind, Checkout, PushRef, Snapshot, Source, changes};
use support::repo::TestRepo;

fn read(repo: &git::Repo, snap: &Snapshot, path: &str) -> String {
    let mut r = BlobReader::new(repo).unwrap();
    String::from_utf8(r.read(&snap.get(path).unwrap().oid).unwrap()).unwrap()
}

#[test]
fn staged_content_wins_over_unstaged_edits() {
    // AE9
    let t = TestRepo::new();
    t.write("a.txt", "one\n");
    t.commit_all("init");
    t.write("a.txt", "staged\n").add(&["a.txt"]);
    t.write("a.txt", "unstaged\n");
    let repo = t.repo();
    let snap = Snapshot::index(&repo).unwrap();
    assert_eq!(read(&repo, &snap, "a.txt"), "staged\n");
    let ch = changes::staged(&repo).unwrap();
    assert_eq!(ch.len(), 1);
    assert_eq!(ch[0].kind, ChangeKind::Modified);
    assert!(changes::unstaged_paths(&repo).unwrap().contains("a.txt"));
}

#[test]
fn whole_file_rename_is_a_rename() {
    let t = TestRepo::new();
    t.write("old.rs", &"fn a() {}\n".repeat(20));
    t.commit_all("init");
    t.git(&["mv", "old.rs", "new.rs"]);
    let ch = changes::staged(&t.repo()).unwrap();
    assert_eq!(ch.len(), 1, "{ch:?}");
    assert_eq!(ch[0].path, "new.rs");
    assert_eq!(
        ch[0].kind,
        ChangeKind::Renamed {
            from: "old.rs".into()
        }
    );
    assert_eq!(ch[0].old_path(), "old.rs");
}

#[test]
fn initial_commit_yields_every_file_as_added() {
    let t = TestRepo::new();
    t.write("a.txt", "a\n")
        .write("dir/b.txt", "b\n")
        .add(&["."]);
    let repo = t.repo();
    assert!(repo.head().is_none());
    let ch = changes::staged(&repo).unwrap();
    let paths: Vec<_> = ch
        .iter()
        .map(|c| (c.path.as_str(), c.kind.clone()))
        .collect();
    assert_eq!(
        paths,
        vec![
            ("a.txt", ChangeKind::Added),
            ("dir/b.txt", ChangeKind::Added)
        ]
    );
    assert!(
        ch.iter()
            .all(|c| c.old_oid.is_none() && c.new_oid.is_some())
    );
}

#[test]
fn push_of_new_branch_diffs_against_remote_default() {
    let t = TestRepo::new();
    let _remote = t.add_remote();
    t.write("base.txt", "base\n");
    t.commit_all("base");
    t.git(&["push", "-q", "--no-verify", "origin", "main"]);
    t.git(&["checkout", "-q", "-b", "feature"]);
    for i in 0..3 {
        t.write(&format!("f{i}.txt"), "x\n");
        t.commit_all(&format!("c{i}"));
    }
    let tip = t.git(&["rev-parse", "HEAD"]);
    t.git(&["checkout", "-q", "main"]);
    let repo = t.repo();
    let push = PushRef {
        local_ref: "refs/heads/feature".into(),
        local_oid: tip.clone(),
        remote_ref: "refs/heads/feature".into(),
        remote_oid: git::ZERO_OID.into(),
    };
    let base = changes::push_base(&repo, "origin", &push).unwrap();
    assert_eq!(base, t.git(&["rev-parse", "main"]));
    let ch = changes::between(&repo, &base, &tip).unwrap();
    let paths: Vec<_> = ch.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["f0.txt", "f1.txt", "f2.txt"]);
    assert_eq!(changes::commits(&repo, Some(&base), &tip).unwrap().len(), 3);
    // The branch is not checked out: the snapshot is its tip, not the working tree.
    let snap = Snapshot::tree(&repo, &tip).unwrap();
    assert!(snap.get("f2.txt").is_some());
    assert!(!t.path().join("f2.txt").exists());
}

#[test]
fn first_push_to_empty_remote_uses_empty_tree() {
    let t = TestRepo::new();
    let _remote = t.add_remote();
    t.write("a.txt", "a\n");
    let tip = t.commit_all("init");
    let repo = t.repo();
    let push = PushRef {
        local_ref: "refs/heads/main".into(),
        local_oid: tip.clone(),
        remote_ref: "refs/heads/main".into(),
        remote_oid: git::ZERO_OID.into(),
    };
    assert_eq!(changes::push_base(&repo, "origin", &push), None);
    let ch = changes::between(&repo, &repo.empty_tree().unwrap(), &tip).unwrap();
    assert_eq!(ch.len(), 1);
    assert_eq!(
        changes::commits(&repo, None, &tip).unwrap(),
        vec![(tip, None)]
    );
}

#[test]
fn pre_push_input_parses_and_flags_deletes() {
    let refs = changes::parse_pre_push(&format!(
        "refs/heads/a 1111 refs/heads/a {z}\n(delete) {z} refs/heads/b 2222\n",
        z = git::ZERO_OID
    ));
    assert_eq!(refs.len(), 2);
    assert!(!refs[0].is_delete());
    assert!(refs[1].is_delete());
}

#[test]
fn checkout_has_staged_content_and_dependency_links() {
    let t = TestRepo::new();
    t.write(".gitignore", "node_modules/\n_build/\n");
    t.write("node_modules/dep/index.js", "x\n");
    t.write("_build/dev/app.beam", "real build\n");
    t.write("src/a.js", "committed\n");
    t.commit_all("init");
    t.write("src/a.js", "staged\n").add(&["src/a.js"]);
    t.write("src/a.js", "unstaged\n");
    let repo = t.repo();
    let co = Checkout::create(&repo, &Source::Index, &[]).unwrap();
    assert!(co.root().starts_with(repo.state_dir()));
    assert_eq!(
        std::fs::read_to_string(co.root().join("src/a.js")).unwrap(),
        "staged\n"
    );
    assert!(co.root().join("node_modules").is_symlink());
    assert!(
        !co.root().join("_build").exists(),
        "build output is never shared with the working tree"
    );
    drop(co);
    let head = Checkout::create(&repo, &Source::Tree("HEAD".into()), &[]).unwrap();
    assert_eq!(
        std::fs::read_to_string(head.root().join("src/a.js")).unwrap(),
        "committed\n"
    );
    assert!(head.root().join("node_modules").is_symlink());
    assert!(
        t.path().join("node_modules/dep/index.js").exists(),
        "symlink target survives"
    );
}

#[test]
fn checkout_is_updated_in_place() {
    let t = TestRepo::new();
    t.write(".gitignore", "_build/\n");
    t.write("lib/same.ex", "same\n")
        .write("lib/edit.ex", "v1\n");
    t.write("lib/gone.ex", "gone\n");
    t.commit_all("init");
    let repo = t.repo();
    let co = Checkout::create(&repo, &Source::Index, &[]).unwrap();
    let root = co.root().to_path_buf();
    let mtime = |p: &str| std::fs::metadata(root.join(p)).unwrap().modified().unwrap();
    let same_before = mtime("lib/same.ex");
    // What a tool leaves behind: build output, a stray file, an edited source.
    std::fs::create_dir_all(root.join("_build")).unwrap();
    std::fs::write(root.join("_build/app.beam"), "built\n").unwrap();
    std::fs::write(root.join("stray.ex"), "stray\n").unwrap();
    std::fs::write(root.join("lib/edit.ex"), "tool edit\n").unwrap();
    drop(co);

    std::thread::sleep(std::time::Duration::from_millis(1100));
    t.write("lib/edit.ex", "v2\n").write("lib/new.ex", "new\n");
    t.git(&["rm", "-q", "lib/gone.ex"]);
    t.add(&["lib/edit.ex", "lib/new.ex"]);
    let co = Checkout::create(&repo, &Source::Index, &[]).unwrap();
    assert_eq!(co.root(), root);
    let read = |p: &str| std::fs::read_to_string(root.join(p)).ok();
    assert_eq!(read("lib/edit.ex").as_deref(), Some("v2\n"));
    assert_eq!(read("lib/new.ex").as_deref(), Some("new\n"));
    assert_eq!(read("lib/gone.ex"), None);
    assert_eq!(read("stray.ex"), None, "untracked files are removed");
    assert_eq!(
        read("_build/app.beam").as_deref(),
        Some("built\n"),
        "ignored build output stays"
    );
    assert_eq!(
        mtime("lib/same.ex"),
        same_before,
        "unchanged files keep their mtime"
    );
}

#[test]
fn checkout_recovers_from_a_damaged_index() {
    let t = TestRepo::new();
    t.write("a.txt", "a\n");
    t.commit_all("init");
    let repo = t.repo();
    drop(Checkout::create(&repo, &Source::Index, &[]).unwrap());
    std::fs::write(repo.state_dir().join("checkout.index"), "garbage").unwrap();
    let co = Checkout::create(&repo, &Source::Index, &[]).unwrap();
    assert_eq!(
        std::fs::read_to_string(co.root().join("a.txt")).unwrap(),
        "a\n"
    );
}

#[test]
fn concurrent_checkouts_wait_for_each_other() {
    let t = TestRepo::new();
    t.write("a.txt", "a\n");
    t.commit_all("init");
    let repo = t.repo();
    let first = Checkout::create(&repo, &Source::Index, &[]).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let second = {
        let repo = repo.clone();
        std::thread::spawn(move || {
            let co = Checkout::create(&repo, &Source::Index, &[]).unwrap();
            tx.send(()).unwrap();
            drop(co);
        })
    };
    let waited = rx.recv_timeout(std::time::Duration::from_millis(300));
    assert!(waited.is_err(), "second run must wait for the lock");
    drop(first);
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .expect("second run proceeds once the first is done");
    second.join().unwrap();
}

#[test]
fn reading_changes_nothing() {
    let t = TestRepo::new();
    t.write("a.txt", "a\n").write("b.txt", "b\n");
    t.commit_all("init");
    t.write("a.txt", "staged\n").add(&["a.txt"]);
    t.write("a.txt", "unstaged\n").write("untracked.txt", "u\n");
    let before = t.fingerprint();
    let repo = t.repo();
    let snap = Snapshot::index(&repo).unwrap();
    read(&repo, &snap, "a.txt");
    changes::staged(&repo).unwrap();
    changes::untracked_paths(&repo).unwrap();
    drop(Checkout::create(&repo, &Source::Index, &[]).unwrap());
    assert_eq!(before, t.fingerprint());
}
