//! The parts of a `mix.exs` that change what the repo's Mix tools check:
//! task aliases, CLI environments, compile and test paths, coverage and
//! Dialyzer settings. Dependency lists and the version are left out, so a
//! dependency bump is not a loosening.

use tree_sitter::Node;

use crate::grammars::{self, Lang};

/// Functions guarded whole: `aliases` can redefine `mix test`; `cli` sets
/// `preferred_envs` on Elixir 1.15+.
const FUNCTIONS: &[&str] = &["aliases", "cli"];

/// Keys of the `project/0` keyword list that are guarded.
const PROJECT_KEYS: &[&str] = &[
    "aliases",
    "elixirc_options",
    "elixirc_paths",
    "test_paths",
    "test_pattern",
    "test_coverage",
    "preferred_cli_env",
    "dialyzer",
];

/// The guarded sections, whitespace-normalised and in source order, or the
/// whole text when it does not parse (so an unparseable edit still counts).
pub fn guarded_sections(text: &str) -> String {
    let Some(parsed) = grammars::parse(Lang::Elixir, text) else {
        return normalize(text);
    };
    if parsed.has_errors {
        return normalize(text);
    }
    let mut out = Vec::new();
    collect(parsed.tree.root_node(), text, &mut out);
    out.join("\n")
}

fn collect(node: Node, src: &str, out: &mut Vec<String>) {
    if let Some(name) = def_name(node, src) {
        if FUNCTIONS.contains(&name) {
            out.push(format!("{name}: {}", tokens(node, src)));
            return;
        }
        if name == "project" {
            project_keys(node, src, out);
            return;
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect(child, src, out);
    }
}

/// `project` in `def project do ... end`, for `def` and `defp` calls.
fn def_name<'a>(node: Node, src: &'a str) -> Option<&'a str> {
    if node.kind() != "call" {
        return None;
    }
    let target = text(node.child_by_field_name("target")?, src);
    if target != "def" && target != "defp" {
        return None;
    }
    let mut cursor = node.walk();
    let args = node
        .named_children(&mut cursor)
        .find(|c| c.kind() == "arguments")?;
    let head = args.named_child(0)?;
    let name = match head.kind() {
        "identifier" => head,
        "call" => head.child_by_field_name("target")?,
        _ => return None,
    };
    Some(text(name, src))
}

/// Guarded `key: value` pairs anywhere in the `project` function.
fn project_keys(node: Node, src: &str, out: &mut Vec<String>) {
    if node.kind() == "pair"
        && let Some(key) = node.child_by_field_name("key")
    {
        let key = text(key, src).trim_end().trim_end_matches(':');
        if PROJECT_KEYS.contains(&key) {
            out.push(tokens(node, src));
            return;
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        project_keys(child, src, out);
    }
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    &src[node.byte_range()]
}

/// The node's tokens separated by spaces, so layout and comments don't count.
fn tokens(node: Node, src: &str) -> String {
    let mut out = Vec::new();
    grammars::for_each_token(node, src, &mut |_, t| out.push(t.trim().to_string()));
    out.join(" ")
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIX: &str = r#"defmodule Elara.MixProject do
  use Mix.Project

  def project do
    [
      app: :elara,
      version: "0.1.0",
      elixirc_paths: elixirc_paths(Mix.env()),
      test_coverage: [summary: [threshold: 90]],
      aliases: aliases(),
      deps: deps()
    ]
  end

  defp deps do
    [{:jason, "~> 1.4"}]
  end

  defp aliases do
    [test: ["ecto.create --quiet", "test"]]
  end
end
"#;

    #[test]
    fn deps_and_version_are_not_guarded() {
        let base = guarded_sections(MIX);
        let bumped = MIX.replace("~> 1.4", "~> 1.5").replace("0.1.0", "0.2.0");
        assert_eq!(guarded_sections(&bumped), base);
        let reformatted = MIX.replace("[test: [", "[\n      test: [");
        assert_eq!(guarded_sections(&reformatted), base);
    }

    #[test]
    fn aliases_and_project_settings_are_guarded() {
        let base = guarded_sections(MIX);
        assert!(base.contains("threshold: 90"), "{base}");
        for edited in [
            MIX.replace(
                "\"ecto.create --quiet\", \"test\"",
                "\"ecto.create --quiet\"",
            ),
            MIX.replace("threshold: 90", "threshold: 10"),
            MIX.replace(
                "      app: :elara,",
                "      app: :elara,\n      test_paths: [],",
            ),
        ] {
            assert_ne!(guarded_sections(&edited), base, "{edited}");
        }
    }
}
