//! The protected-path list (KTD7), shared by file-editing tools and shell
//! write, move and delete targets: the git hooks, `.git/config`, `.git/info`,
//! the approval records, and project and user `.claude/settings*.json`.

use std::path::{Component, Path, PathBuf};

use crate::git::Repo;

/// How a path is touched: written in place, or removed or moved (which also
/// covers every protected path inside it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Write,
    Remove,
}

/// The protected locations for one repository and user.
#[derive(Clone, Debug, Default)]
pub struct Guard {
    /// Protected files and directories (everything inside is protected too).
    roots: Vec<PathBuf>,
    /// Directories whose `settings*.json` files are protected.
    settings_dirs: Vec<PathBuf>,
    /// The repository's working-tree root, when there is one.
    pub repo_root: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// Lowercase text that marks an argument as touching the gate.
    pub markers: Vec<String>,
}

/// The canonical path of the running qgate binary.
pub fn self_exe() -> Option<PathBuf> {
    std::env::current_exe().ok()?.canonicalize().ok()
}

/// Whether `program` (as typed, resolved against `cwd` or `PATH`) is this
/// qgate binary under any name, such as a symlink or renamed copy on PATH.
pub fn is_self(program: &str, cwd: &Path) -> bool {
    let Some(exe) = self_exe() else {
        return false;
    };
    let same = |p: &Path| p.canonicalize().is_ok_and(|c| c == exe);
    if program.contains('/') {
        return same(&cwd.join(program));
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|d| same(&d.join(program))))
}

impl Guard {
    /// The guard for the repository containing `cwd` (if any) and `$HOME`.
    pub fn for_dir(cwd: &Path) -> Guard {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from);
        let mut guard = Guard {
            home: home.clone(),
            markers: [
                ".git/hooks",
                ".git/config",
                ".git/info",
                ".git/qgate",
                ".qgate/approvals",
                ".qgate/allowed_signers",
                ".claude/settings",
                "no-verify",
                "hookspath",
                "git_config_count",
                "git_config_key_",
                "git_config_parameters",
            ]
            .map(String::from)
            .to_vec(),
            ..Guard::default()
        };
        if let Some(home) = &home {
            guard.add_settings_dir(home.join(".claude"));
            // User-level git config can redirect or disable hooks for every repo.
            guard.add_root(home.join(".gitconfig"));
            // Protect ~/.config/git even when XDG_CONFIG_HOME points elsewhere:
            // git falls back to it whenever that variable is unset.
            guard.add_root(home.join(".config/git"));
            if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
                guard.add_root(PathBuf::from(xdg).join("git"));
            }
        }
        guard.add_root(PathBuf::from("/etc/gitconfig"));
        guard
            .markers
            .extend([".gitconfig".to_string(), ".config/git".to_string()]);
        // Every hook runs this binary by absolute path; replacing it disables them all.
        if let Some(exe) = self_exe() {
            guard.markers.push(exe.to_string_lossy().to_lowercase());
            guard.add_root(exe);
        }
        if let Ok(repo) = Repo::discover(cwd) {
            guard.add_repo(&repo);
        }
        guard
    }

    fn add_root(&mut self, p: PathBuf) {
        let canon = canonical(&p);
        if canon != p {
            self.roots.push(canon);
        }
        self.roots.push(p);
    }

    fn add_settings_dir(&mut self, p: PathBuf) {
        let canon = canonical(&p);
        if canon != p {
            self.settings_dirs.push(canon);
        }
        self.settings_dirs.push(p);
    }

    fn add_repo(&mut self, repo: &Repo) {
        let common = repo.git_dir.clone();
        for sub in ["hooks", "config", "info", "qgate/approvals"] {
            self.add_root(common.join(sub));
        }
        if let Some(dir) = repo.try_output(&["rev-parse", "--absolute-git-dir"]) {
            let dir = PathBuf::from(dir);
            for sub in ["config", "config.worktree", "info"] {
                self.add_root(dir.join(sub));
            }
        }
        if let Ok(hooks) = repo.hooks_dir() {
            let hooks = normalize(&hooks);
            if !hooks.starts_with(&common) {
                self.markers.push(hooks.to_string_lossy().to_lowercase());
            }
            self.add_root(hooks);
        }
        if common.file_name().is_some_and(|n| n != ".git") {
            self.markers.push(common.to_string_lossy().to_lowercase());
        }
        self.add_settings_dir(repo.root.join(".claude"));
        // Committed approval records and the signer list. The gate only trusts
        // signers from the tree before a change, so this is defense in depth.
        self.add_root(repo.root.join(".qgate"));
        let signers = std::fs::read_to_string(repo.root.join(crate::config::CONFIG_FILE))
            .ok()
            .and_then(|t| crate::config::Config::parse_unchecked(&t, "qgate.toml").ok())
            .and_then(|c| c.approvals.allowed_signers);
        if let Some(p) = signers {
            self.markers.push(p.to_lowercase());
            self.add_root(repo.root.join(p));
        }
        self.repo_root = Some(repo.root.clone());
    }

    /// Whether writing `p` (absolute, normalized) touches a protected path.
    pub fn is_protected(&self, p: &Path) -> bool {
        lexically_protected(p)
            || self.roots.iter().any(|r| p.starts_with(r))
            || self.settings_dirs.iter().any(|d| {
                p.parent() == Some(d.as_path())
                    && p.file_name()
                        .is_some_and(|n| is_settings_name(&n.to_string_lossy()))
            })
    }

    /// Whether removing or moving `p` takes a protected path with it.
    pub fn contains_protected(&self, p: &Path) -> bool {
        if self.is_protected(p) {
            return true;
        }
        let last = p.file_name().map(|n| n.to_string_lossy().into_owned());
        matches!(last.as_deref(), Some(".git" | ".claude"))
            || p.ends_with(".git/qgate")
            || self.roots.iter().any(|r| r.starts_with(p))
            || self.settings_dirs.iter().any(|d| d.starts_with(p))
    }

    /// Whether one path argument, resolved against `cwd`, names a protected
    /// location. Symlinks in existing parents and glob patterns are followed.
    pub fn check(&self, raw: &str, cwd: &Path, access: Access) -> bool {
        if raw.is_empty() {
            return false;
        }
        let test = |p: &Path| match access {
            Access::Write => self.is_protected(p),
            Access::Remove => self.contains_protected(p),
        };
        let joined = cwd.join(raw);
        let lexical = normalize(&joined);
        if test(&lexical) || test(&canonical(&joined)) || test(&canonical(&lexical)) {
            return true;
        }
        if raw.contains(['*', '?', '[', '{']) {
            return self.glob_hits(&lexical, access);
        }
        false
    }

    /// Whether a glob pattern matches any protected location (or, for
    /// removal, any directory holding one).
    fn glob_hits(&self, pattern: &Path, access: Access) -> bool {
        let Ok(glob) = globset::GlobBuilder::new(&pattern.to_string_lossy())
            .literal_separator(true)
            .build()
        else {
            return true;
        };
        let matcher = glob.compile_matcher();
        let mut candidates: Vec<PathBuf> = Vec::new();
        for dir in self.roots.iter().chain(&self.settings_dirs) {
            candidates.push(dir.clone());
            if let Ok(entries) = std::fs::read_dir(dir) {
                candidates.extend(entries.flatten().map(|e| e.path()));
            }
            let mut up = dir.parent();
            while let Some(a) = up {
                candidates.push(a.to_path_buf());
                up = a.parent();
            }
        }
        candidates.iter().any(|c| {
            matcher.is_match(c)
                && match access {
                    Access::Write => self.is_protected(c),
                    Access::Remove => self.contains_protected(c),
                }
        })
    }

    /// The first marker (protected path fragment or bypass flag) in `text`.
    pub fn marker_in(&self, text: &str) -> Option<&str> {
        let lc = text.to_lowercase();
        self.markers
            .iter()
            .find(|m| lc.contains(m.as_str()))
            .map(String::as_str)
    }
}

fn is_settings_name(name: &str) -> bool {
    name.starts_with("settings") && name.ends_with(".json")
}

/// Protected by name alone, wherever the repository is: anything under a
/// `.git` directory's hooks, config, info or approvals, and any
/// `.claude/settings*.json`.
fn lexically_protected(p: &Path) -> bool {
    let comps: Vec<String> = p
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let n = comps.len();
    if n >= 2 && comps[n - 2] == ".claude" && is_settings_name(&comps[n - 1]) {
        return true;
    }
    comps.iter().enumerate().any(|(i, c)| {
        c == ".git"
            && match &comps[i + 1..] {
                [first, ..] if first == "hooks" || first == "info" => true,
                [only] => only == "config" || only == "config.worktree",
                [q, a, ..] => q == "qgate" && a == "approvals",
                _ => false,
            }
    })
}

/// Resolve `.` and `..` without touching the filesystem.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// Resolve symlinks in the longest existing prefix of `p`, keeping the rest.
pub fn canonical(p: &Path) -> PathBuf {
    let mut existing = p;
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(c) = existing.canonicalize() {
            let mut out = c;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return normalize(&out);
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return normalize(p),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_rules() {
        for p in [
            "/r/.git/hooks/pre-commit",
            "/r/.git/hooks",
            "/r/.git/config",
            "/r/.git/info/exclude",
            "/r/.git/qgate/approvals/a",
            "/r/.claude/settings.json",
            "/r/sub/.claude/settings.local.json",
        ] {
            assert!(lexically_protected(Path::new(p)), "{p}");
        }
        for p in [
            "/r/.git/HEAD",
            "/r/.git/qgate/cache",
            "/r/src/main.rs",
            "/r/.claude/agents/x.md",
        ] {
            assert!(!lexically_protected(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn normalize_resolves_dots() {
        assert_eq!(
            normalize(Path::new("/r/a/../.git/./hooks")),
            PathBuf::from("/r/.git/hooks")
        );
    }

    #[test]
    fn removal_covers_parents() {
        let g = Guard::default();
        assert!(g.contains_protected(Path::new("/r/.git")));
        assert!(g.contains_protected(Path::new("/r/.claude")));
        assert!(!g.contains_protected(Path::new("/r/target")));
    }
}
