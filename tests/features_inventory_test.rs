mod support;

use qgate::checks::exclude::Excludes;
use qgate::config::Config;
use qgate::features::inventory::{EntryPoint, Kind, Scope, Tree, inventory};
use qgate::git::Snapshot;
use support::repo::TestRepo;

/// Stage `files` in a new repo and inventory the index.
fn entries_with(
    files: &[(&str, &str)],
    config: &Config,
    scope: Option<&[&str]>,
) -> Vec<EntryPoint> {
    let t = TestRepo::new();
    for (path, content) in files {
        t.write(path, content);
    }
    t.git(&["add", "-A"]);
    let repo = t.repo();
    let snapshot = Snapshot::index(&repo).unwrap();
    let excludes = Excludes::new(&repo, &snapshot, config).unwrap();
    let tree = Tree::new(&snapshot, &excludes);
    let paths: Vec<String> = scope
        .unwrap_or_default()
        .iter()
        .map(|p| p.to_string())
        .collect();
    let scope = match scope {
        Some(_) => Scope::Paths(&paths),
        None => Scope::All,
    };
    inventory(&repo, &tree, scope).unwrap()
}

fn entries(files: &[(&str, &str)]) -> Vec<EntryPoint> {
    entries_with(files, &Config::starter(), None)
}

/// `kind name path[:line]` per entry point.
fn names(entries: &[EntryPoint]) -> Vec<String> {
    entries
        .iter()
        .map(|e| match e.line {
            Some(l) => format!("{} {} {}:{l}", e.kind.as_str(), e.name, e.path),
            None => format!("{} {} {}", e.kind.as_str(), e.name, e.path),
        })
        .collect()
}

fn of_kind(entries: &[EntryPoint], kind: Kind) -> Vec<String> {
    let only: Vec<EntryPoint> = entries.iter().filter(|e| e.kind == kind).cloned().collect();
    names(&only)
}

const CARGO: &str = "[package]\nname = \"shop\"\nversion = \"0.1.0\"\n";

#[test]
fn cargo_binaries_come_from_main_bin_and_bin_tables() {
    let e = entries(&[
        (
            "Cargo.toml",
            &format!("{CARGO}\n[[bin]]\nname = \"gen\"\npath = \"tools/gen.rs\"\n"),
        ),
        ("src/main.rs", "fn main() {}\n"),
        ("src/bin/sync.rs", "fn main() {}\n"),
        ("src/bin/admin/main.rs", "fn main() {}\n"),
        ("src/bin/admin/args.rs", "\n"),
        ("tools/gen.rs", "fn main() {}\n"),
    ]);
    assert_eq!(
        of_kind(&e, Kind::Binary),
        [
            "binary admin src/bin/admin/main.rs",
            "binary sync src/bin/sync.rs",
            "binary shop src/main.rs",
            "binary gen tools/gen.rs",
        ]
    );
    assert!(
        !names(&e).iter().any(|n| n.starts_with("module bin ")),
        "src/bin is not a module: {e:?}"
    );
}

#[test]
fn literal_routes_are_found_with_their_lines() {
    let src = "fn app() -> Router {\n    Router::new()\n        .route(\"/health\", get(h))\n        .route(\"/api/books\", post(c))\n        .route(path_var, get(x))\n}\n";
    let e = entries(&[("Cargo.toml", CARGO), ("src/lib.rs", src)]);
    assert_eq!(
        of_kind(&e, Kind::Route),
        [
            "route /api/books src/lib.rs:4",
            "route /health src/lib.rs:3"
        ]
    );
}

#[test]
fn subcommand_variants_are_commands_in_kebab_case() {
    let src = "#[derive(Debug, Subcommand)]\npub enum Command {\n    /// Install.\n    Install,\n    Run(RunArgs),\n    AgentHook,\n    #[command(name = \"self-update\")]\n    Update,\n}\n\n#[derive(Debug)]\nenum Other {\n    Plain,\n}\n";
    let e = entries(&[("Cargo.toml", CARGO), ("src/lib.rs", src)]);
    assert_eq!(
        of_kind(&e, Kind::Command),
        [
            "command agent-hook src/lib.rs:6",
            "command install src/lib.rs:4",
            "command run src/lib.rs:5",
            "command self-update src/lib.rs:8",
        ]
    );
}

#[test]
fn migrations_in_any_language() {
    let e = entries(&[
        ("migrations/V3__books.sql", "create table b();\n"),
        ("migrations/README.md", "notes\n"),
        ("priv/repo/migrations/2026_add.exs", "defmodule M do end\n"),
        ("alembic/versions/ab12_init.py", "x = 1\n"),
        ("alembic/versions/__init__.py", ""),
    ]);
    assert_eq!(
        of_kind(&e, Kind::Migration),
        [
            "migration ab12_init.py alembic/versions/ab12_init.py",
            "migration V3__books.sql migrations/V3__books.sql",
            "migration 2026_add.exs priv/repo/migrations/2026_add.exs",
        ]
    );
}

#[test]
fn modules_are_children_of_source_roots() {
    let rust = entries(&[
        ("Cargo.toml", CARGO),
        ("src/lib.rs", "\n"),
        ("src/hash.rs", "\n"),
        ("src/baseline/mod.rs", "\n"),
    ]);
    assert_eq!(
        of_kind(&rust, Kind::Module),
        ["module baseline src/baseline", "module hash src/hash.rs"]
    );
    let elixir = entries(&[
        ("mix.exs", "defmodule P do end\n"),
        ("lib/elara.ex", "\n"),
        ("lib/elara/repo.ex", "\n"),
        ("lib/elara/accounts/user.ex", "\n"),
    ]);
    assert_eq!(
        of_kind(&elixir, Kind::Module),
        [
            "module accounts lib/elara/accounts",
            "module repo lib/elara/repo.ex"
        ]
    );
    let go = entries(&[
        ("go.mod", "module x\n"),
        ("cmd/server/main.go", "package main\n"),
        ("internal/books/books.go", "package books\n"),
        ("docs/readme.md", "\n"),
    ]);
    assert_eq!(
        of_kind(&go, Kind::Module),
        ["module server cmd/server", "module books internal/books"]
    );
}

#[test]
fn test_example_and_excluded_files_yield_nothing() {
    let mut config = Config::starter();
    config.exclude = vec!["src/gen/**".into()];
    let e = entries_with(
        &[
            ("Cargo.toml", CARGO),
            ("src/lib.rs", "\n"),
            (
                "tests/main.rs",
                "fn main() {}\n#[derive(Subcommand)]\nenum C { A }\n",
            ),
            ("examples/demo.rs", "fn main() {}\n"),
            ("src/gen/api.rs", "fn r() { x.route(\"/gen\", h); }\n"),
        ],
        &config,
        None,
    );
    assert!(e.is_empty(), "{e:?}");
}

#[test]
fn a_scope_limits_entries_to_the_paths_given() {
    let files = [
        ("Cargo.toml", CARGO),
        ("src/main.rs", "fn main() {}\n"),
        ("src/a/x.rs", "\n"),
        ("src/b.rs", "\n"),
        ("migrations/V1__a.sql", "\n"),
    ];
    let e = entries_with(
        &files,
        &Config::starter(),
        Some(&["src/a/x.rs", "migrations/V1__a.sql"]),
    );
    assert_eq!(
        names(&e),
        ["migration V1__a.sql migrations/V1__a.sql", "module a src/a"]
    );
}
