//! Temporary git repositories for tests. Every test that touches git builds one.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

pub struct TestRepo {
    dir: TempDir,
}

/// A git command isolated from the developer's global and system config.
pub fn git_cmd(cwd: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_DIR");
    cmd
}

impl TestRepo {
    pub fn new() -> TestRepo {
        let repo = TestRepo {
            dir: tempfile::Builder::new()
                .prefix("qgate-test-")
                .tempdir()
                .unwrap(),
        };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn repo(&self) -> qgate::git::Repo {
        qgate::git::Repo::discover(self.path()).unwrap()
    }

    pub fn write(&self, rel: &str, content: &str) -> &Self {
        let p = self.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
        self
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path().join(rel)).unwrap()
    }

    pub fn remove(&self, rel: &str) {
        std::fs::remove_file(self.path().join(rel)).unwrap();
    }

    pub fn git_output(&self, args: &[&str]) -> Output {
        git_cmd(self.path()).args(args).output().unwrap()
    }

    /// Run git, panic on failure, return trimmed stdout.
    pub fn git(&self, args: &[&str]) -> String {
        let out = self.git_output(args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    pub fn add(&self, paths: &[&str]) -> &Self {
        let mut args = vec!["add", "--"];
        args.extend(paths);
        self.git(&args);
        self
    }

    /// Stage everything and commit, without running hooks. Returns the commit id.
    pub fn commit_all(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--no-verify", "--allow-empty", "-m", msg]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// `git status` plus a hash of every working-tree file and the index,
    /// for asserting a run changed nothing.
    pub fn fingerprint(&self) -> String {
        let status = self.git(&["status", "--porcelain=v1", "-uall"]);
        // Staged entries, not raw index bytes: git itself refreshes stat data.
        let index = self.git(&["ls-files", "--stage"]);
        let mut files: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        collect(self.path(), self.path(), &mut files);
        files.sort();
        format!("{status}\n{files:?}\n{index}")
    }

    /// A bare repo to push to, registered as `origin`.
    pub fn add_remote(&self) -> TempDir {
        let bare = tempfile::Builder::new()
            .prefix("qgate-remote-")
            .tempdir()
            .unwrap();
        let out = git_cmd(bare.path())
            .args(["init", "-q", "--bare", "-b", "main"])
            .output()
            .unwrap();
        assert!(out.status.success());
        self.git(&["remote", "add", "origin", bare.path().to_str().unwrap()]);
        bare
    }

    /// A `qgate` command running in this repo with isolated git config.
    pub fn qgate(&self) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::cargo_bin("qgate").unwrap();
        cmd.current_dir(self.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_DIR");
        cmd
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            out.push((
                path.strip_prefix(root).unwrap().to_path_buf(),
                std::fs::read(&path).unwrap(),
            ));
        }
    }
}
