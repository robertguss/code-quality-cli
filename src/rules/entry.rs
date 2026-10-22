//! Binary entry points and scripts, where printing is the program's job (KTD9).

use tree_sitter::Tree;

use crate::grammars::Lang;

/// Whether `path` is a binary's main file or a script, so debug-output rules
/// that only flag stray printing skip it.
pub fn is_entry(path: &str, lang: Lang, tree: &Tree, src: &str) -> bool {
    let segments: Vec<&str> = path.split('/').collect();
    let (name, dirs) = segments.split_last().unwrap_or((&"", &[]));
    if dirs.contains(&"scripts") {
        return true;
    }
    match lang {
        Lang::Rust => {
            *name == "build.rs"
                || (*name == "main.rs" && dirs.last() == Some(&"src"))
                || dirs.windows(2).any(|w| w == ["src", "bin"])
        }
        Lang::Go => is_package_main(tree, src),
        Lang::Python => *name == "__main__.py",
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => dirs.contains(&"bin"),
        Lang::Elixir => dirs.windows(3).any(|w| w == ["lib", "mix", "tasks"]),
        _ => false,
    }
}

/// A Go file whose package clause is `package main`.
pub fn is_package_main(tree: &Tree, src: &str) -> bool {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let found = root
        .named_children(&mut cursor)
        .find(|n| n.kind() == "package_clause")
        .and_then(|clause| {
            let mut c = clause.walk();
            clause
                .named_children(&mut c)
                .find(|n| n.kind() == "package_identifier")
                .map(|id| &src[id.byte_range()] == "main")
        });
    found.unwrap_or(false)
}
