//! Rust entry points (KTD14): Cargo binaries, axum-style `.route("/path", …)`
//! calls and clap `Subcommand` variants.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use tree_sitter::Node;

use super::{EntryPoint, Kind};
use crate::grammars::{self, Lang};

/// The binaries a crate builds, as `(path, name)`, from its `Cargo.toml`
/// text and the tree's paths. `dir` is the crate's directory.
pub fn binaries(dir: &str, manifest: &str, paths: &[&str]) -> Vec<(String, String)> {
    let at = |rel: &str| {
        if dir.is_empty() {
            rel.to_string()
        } else {
            format!("{dir}/{rel}")
        }
    };
    let has_file = |p: &str| paths.contains(&p);
    let table: toml::Table = toml::from_str(manifest).unwrap_or_default();
    let package = table
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str());
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    if let Some(name) = package
        && has_file(&at("src/main.rs"))
    {
        out.insert(at("src/main.rs"), name.to_string());
    }
    if package.is_some() {
        out.extend(auto_bins(&at("src/bin/"), paths));
    }
    let explicit = table.get("bin").and_then(|b| b.as_array());
    for bin in explicit.into_iter().flatten() {
        let name = bin.get("name").and_then(|n| n.as_str());
        let path = bin.get("path").and_then(|p| p.as_str()).map(at);
        match (name, path) {
            (Some(name), Some(path)) => {
                out.insert(path, name.to_string());
            }
            (None, Some(path)) => {
                let stem = file_stem(&path);
                out.insert(path, stem);
            }
            _ => {}
        }
    }
    out.into_iter().collect()
}

/// `src/bin/<name>.rs` and `src/bin/<name>/main.rs`.
fn auto_bins(bin_dir: &str, paths: &[&str]) -> Vec<(String, String)> {
    paths
        .iter()
        .filter_map(|p| {
            let rest = p.strip_prefix(bin_dir)?;
            let name = match rest.split_once('/') {
                Some((name, "main.rs")) => name.to_string(),
                None if rest.ends_with(".rs") => file_stem(rest),
                _ => return None,
            };
            Some((p.to_string(), name))
        })
        .collect()
}

fn file_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(s, _)| s).to_string()
}

/// Routes and commands declared in one Rust file.
pub fn declared(path: &str, source: &str) -> Vec<EntryPoint> {
    let Some(parsed) = grammars::parse(Lang::Rust, source) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk(parsed.tree.root_node(), source, path, &mut out);
    out
}

fn walk(node: Node, src: &str, path: &str, out: &mut Vec<EntryPoint>) {
    match node.kind() {
        "call_expression" => {
            if let Some(e) = route(node, src, path) {
                out.push(e);
            }
        }
        "enum_item" if derives_subcommand(node, src) => commands(node, src, path, out),
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, src, path, out);
    }
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    &src[node.byte_range()]
}

fn line(node: Node) -> Option<u32> {
    Some(node.start_position().row as u32 + 1)
}

/// `<expr>.route("<literal>", …)`.
fn route(call: Node, src: &str, path: &str) -> Option<EntryPoint> {
    let function = call.child_by_field_name("function")?;
    let field = function.child_by_field_name("field")?;
    if function.kind() != "field_expression" || text(field, src) != "route" {
        return None;
    }
    let args = call.child_by_field_name("arguments")?;
    let first = args.named_child(0)?;
    if first.kind() != "string_literal" {
        return None;
    }
    let literal = text(first, src);
    let name = literal.strip_prefix('"')?.strip_suffix('"')?;
    Some(EntryPoint {
        kind: Kind::Route,
        path: path.to_string(),
        name: name.to_string(),
        line: line(field),
    })
}

/// The attributes written right before `node`, skipping comments.
fn attributes<'a>(node: Node, src: &'a str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut prev = node.prev_named_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" => out.push(text(p, src)),
            "line_comment" | "block_comment" => {}
            _ => break,
        }
        prev = p.prev_named_sibling();
    }
    out
}

fn derives_subcommand(node: Node, src: &str) -> bool {
    attributes(node, src).iter().any(|a| {
        a.contains("derive")
            && a.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|w| w == "Subcommand")
    })
}

fn rename_pattern() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r#"\bname\s*=\s*"([^"]+)""#).expect("valid regex"))
}

fn commands(item: Node, src: &str, path: &str, out: &mut Vec<EntryPoint>) {
    let Some(body) = item.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for variant in body.named_children(&mut cursor) {
        if variant.kind() != "enum_variant" {
            continue;
        }
        let Some(ident) = variant.child_by_field_name("name") else {
            continue;
        };
        let renamed = attributes(variant, src)
            .iter()
            .find_map(|a| rename_pattern().captures(a))
            .map(|c| c[1].to_string());
        out.push(EntryPoint {
            kind: Kind::Command,
            path: path.to_string(),
            name: renamed.unwrap_or_else(|| kebab(text(ident, src))),
            line: line(variant),
        });
    }
}

/// clap's default command name: `AgentHook` becomes `agent-hook`.
fn kebab(ident: &str) -> String {
    let chars: Vec<char> = ident.chars().collect();
    let mut out = String::new();
    for (i, c) in chars.iter().enumerate() {
        if c.is_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || prev.is_ascii_digit() || (prev.is_uppercase() && next_lower) {
                out.push('-');
            }
        }
        out.extend(c.to_lowercase());
    }
    out
}
