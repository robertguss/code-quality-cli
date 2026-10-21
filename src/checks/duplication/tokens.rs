//! Normalized tokens: whitespace and comments dropped, identifiers kept.

use crate::grammars::{self, Lang};
use crate::hash::fnv64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub hash: u64,
    pub line: u32,
}

/// Extensions without a bundled grammar that are still checked for
/// duplication with the generic lexer.
const GENERIC_CODE: &[&str] = &[
    "kt", "kts", "swift", "scala", "lua", "erl", "hs", "ml", "mli", "clj", "cljs", "dart", "r",
    "pl", "pm", "vue", "svelte", "zig", "nim", "cr", "groovy", "m", "mm", "sql", "css", "scss",
    "less", "html", "fs", "jl", "elm", "tf",
];

/// Generic-lexer languages whose line comments start with `#`.
const HASH_COMMENTS: &[&str] = &["r", "pl", "pm", "nim", "cr", "jl", "tf"];

/// Generic-lexer languages whose line comments start with `--`.
const DASH_COMMENTS: &[&str] = &["lua", "hs", "sql", "elm"];

fn ext(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Whether the duplication check looks at this file at all.
pub fn is_code(path: &str) -> bool {
    Lang::detect(path).is_some() || GENERIC_CODE.contains(&ext(path).as_str())
}

/// A stable tag for how `path` is tokenized, part of the cache key.
pub fn tokenizer_tag(path: &str) -> String {
    match Lang::detect(path) {
        Some(lang) => lang.name().to_string(),
        None => format!("generic-{}", ext(path)),
    }
}

fn hash_text(t: &str) -> u64 {
    fnv64(t.as_bytes())
}

pub fn tokenize(path: &str, text: &str) -> Vec<Token> {
    if let Some(lang) = Lang::detect(path)
        && let Some(parsed) = grammars::parse(lang, text)
    {
        let mut out = Vec::new();
        grammars::for_each_token(parsed.tree.root_node(), text, &mut |node, t| {
            out.push(Token {
                hash: hash_text(t),
                line: node.start_position().row as u32 + 1,
            });
        });
        return out;
    }
    generic(&ext(path), text)
}

/// A small lexer for languages without a bundled grammar.
fn generic(ext: &str, text: &str) -> Vec<Token> {
    let hash_comments = HASH_COMMENTS.contains(&ext);
    let dash_comments = DASH_COMMENTS.contains(&ext);
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut line = 1u32;
    let mut i = 0;
    let push = |out: &mut Vec<Token>, s: &[u8], line: u32| {
        out.push(Token {
            hash: hash_text(&String::from_utf8_lossy(s)),
            line,
        })
    };
    while i < b.len() {
        let c = b[i];
        let rest = &b[i..];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if rest.starts_with(b"//")
            || (hash_comments && c == b'#')
            || (dash_comments && rest.starts_with(b"--"))
        {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if rest.starts_with(b"/*") || rest.starts_with(b"<!--") {
            let end: &[u8] = if c == b'/' { b"*/" } else { b"-->" };
            i += 2;
            while i < b.len() && !b[i..].starts_with(end) {
                if b[i] == b'\n' {
                    line += 1;
                }
                i += 1;
            }
            i = (i + end.len()).min(b.len());
        } else if c == b'"' || c == b'\'' || c == b'`' {
            let start = i;
            let start_line = line;
            i += 1;
            while i < b.len() && b[i] != c {
                if b[i] == b'\\' {
                    i += 1;
                } else if b[i] == b'\n' {
                    line += 1;
                }
                i += 1;
            }
            i = (i + 1).min(b.len());
            push(&mut out, &b[start..i], start_line);
        } else if c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80 {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 0x80) {
                i += 1;
            }
            push(&mut out, &b[start..i], line);
        } else {
            push(&mut out, &b[i..i + 1], line);
            i += 1;
        }
    }
    out
}
