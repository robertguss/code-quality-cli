//! Turn tool output in a known format into per-line findings.

mod mix;

use serde_json::Value;

use crate::config::ToolFormat;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
}

/// One problem a tool reported, before paths are made repo-relative.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    /// The tool's own rule id, when it gave one.
    pub rule: Option<String>,
    pub level: Level,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub message: String,
}

/// Parse a tool's output. Empty means nothing was recognized.
pub fn parse(format: ToolFormat, stdout: &str, stderr: &str) -> Vec<Parsed> {
    let (stdout, stderr) = (strip_ansi(stdout), strip_ansi(stderr));
    let all = format!("{stdout}\n{stderr}");
    match format {
        ToolFormat::Sarif => sarif(&stdout).unwrap_or_default(),
        ToolFormat::EslintJson => eslint_json(&stdout).unwrap_or_default(),
        ToolFormat::Mix => mix::parse(&all),
        ToolFormat::Gcc => gcc(&all),
        ToolFormat::Text => Vec::new(),
        ToolFormat::Auto => sarif(&stdout)
            .or_else(|| eslint_json(&stdout))
            .or_else(|| Some(mix::parse(&all)).filter(|m| !m.is_empty()))
            .unwrap_or_else(|| gcc(&all)),
    }
}

/// Remove ANSI escape sequences (colours, cursor moves) that some tools print
/// even when their output is not a terminal.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: parameters and intermediates, then one final byte.
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC \.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// The first JSON value in `text`, allowing leading lines of noise (such as
/// npm's script banner) and trailing text.
fn first_json(text: &str) -> Option<Value> {
    let first = |s: &str| {
        serde_json::Deserializer::from_str(s)
            .into_iter::<Value>()
            .next()
            .and_then(Result::ok)
    };
    let trimmed = text.trim_start();
    if trimmed.starts_with(['{', '[']) {
        return first(trimmed);
    }
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with(['{', '[']) {
            return first(&text[offset..]);
        }
        offset += line.len();
    }
    None
}

/// SARIF 2.1: `runs[].results[]`. `None` when the text is not SARIF.
pub fn sarif(text: &str) -> Option<Vec<Parsed>> {
    let doc = first_json(text)?;
    let runs = doc.get("runs")?.as_array()?;
    let mut out = Vec::new();
    for result in runs
        .iter()
        .filter_map(|r| r.get("results")?.as_array())
        .flatten()
    {
        let level = match result.get("level").and_then(Value::as_str) {
            Some("error") => Level::Error,
            _ => Level::Warning,
        };
        let rule = result
            .get("ruleId")
            .or_else(|| result.pointer("/rule/id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let message = result
            .pointer("/message/text")
            .and_then(Value::as_str)
            .unwrap_or("(no message)")
            .to_string();
        let loc = result.pointer("/locations/0/physicalLocation");
        let path = loc
            .and_then(|l| l.pointer("/artifactLocation/uri"))
            .and_then(Value::as_str)
            .map(|u| u.strip_prefix("file://").unwrap_or(u).to_string());
        let line = loc
            .and_then(|l| l.pointer("/region/startLine"))
            .and_then(Value::as_u64)
            .map(|l| l as u32);
        out.push(Parsed {
            rule,
            level,
            path,
            line,
            message,
        });
    }
    Some(out)
}

/// ESLint's `--format json`: `[{filePath, messages: [{ruleId, severity, message, line}]}]`.
pub fn eslint_json(text: &str) -> Option<Vec<Parsed>> {
    let doc = first_json(text)?;
    let files = doc.as_array()?;
    let mut out = Vec::new();
    for file in files {
        let path = file.get("filePath")?.as_str()?;
        for m in file.get("messages")?.as_array()? {
            out.push(Parsed {
                rule: m.get("ruleId").and_then(Value::as_str).map(str::to_string),
                level: if m.get("severity").and_then(Value::as_u64) == Some(2) {
                    Level::Error
                } else {
                    Level::Warning
                },
                path: Some(path.to_string()),
                line: m.get("line").and_then(Value::as_u64).map(|l| l as u32),
                message: m
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("(no message)")
                    .to_string(),
            });
        }
    }
    Some(out)
}

/// Lines like `path:line[:col]: [error|warning|note:] message`, as printed by
/// gcc, clang, ruff, mypy and go vet. Notes are context, so they are dropped.
pub fn gcc(text: &str) -> Vec<Parsed> {
    text.lines().filter_map(gcc_line).collect()
}

fn gcc_line(line: &str) -> Option<Parsed> {
    let (path, rest) = line.split_once(':')?;
    if path.is_empty() || path.contains(char::is_whitespace) {
        return None;
    }
    let (num, mut rest) = rest.split_once(':')?;
    let line_no: u32 = all_digits(num)?.parse().ok()?;
    if let Some((col, after)) = rest.split_once(':')
        && all_digits(col).is_some()
    {
        rest = after;
    }
    if !rest.starts_with(' ') {
        return None;
    }
    let mut message = rest.trim();
    let mut level = Level::Error;
    for (prefix, lvl) in [
        ("fatal error:", Some(Level::Error)),
        ("error:", Some(Level::Error)),
        ("warning:", Some(Level::Warning)),
        ("note:", None),
        // Erlang capitalises them.
        ("Error:", Some(Level::Error)),
        ("Warning:", Some(Level::Warning)),
    ] {
        if let Some(m) = message.strip_prefix(prefix) {
            level = lvl?;
            message = m.trim();
            break;
        }
    }
    if message.is_empty() {
        return None;
    }
    let mut rule = None;
    // mypy and gcc put the rule last: `... [attr-defined]`, `[-Wunused]`.
    if let Some(open) = message.rfind(" [")
        && let Some(code) = message[open + 2..].strip_suffix(']')
        && !code.is_empty()
        && !code.contains(char::is_whitespace)
    {
        rule = Some(code.to_string());
        message = message[..open].trim_end();
    }
    // ruff and flake8 put it first: `F401 ...`.
    if rule.is_none()
        && let Some((code, msg)) = message.split_once(' ')
        && is_code(code)
    {
        rule = Some(code.to_string());
        message = msg.trim_start_matches("[*]").trim();
    }
    Some(Parsed {
        rule,
        level,
        path: Some(path.to_string()),
        line: Some(line_no),
        message: message.to_string(),
    })
}

fn all_digits(s: &str) -> Option<&str> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then_some(s)
}

/// An upper-case letter prefix then digits, such as `E501` or `PLR0913`.
fn is_code(s: &str) -> bool {
    let letters = s.bytes().take_while(u8::is_ascii_uppercase).count();
    let digits = s.len() - letters;
    letters > 0 && digits > 0 && s.len() <= 10 && s[letters..].bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sarif_results_become_findings() {
        let text = r#"{"version":"2.1.0","runs":[{"results":[
            {"ruleId":"no-eval","level":"error","message":{"text":"eval is bad"},
             "locations":[{"physicalLocation":{"artifactLocation":{"uri":"src/a.js"},"region":{"startLine":4}}}]},
            {"message":{"text":"meh"}}]}]}"#;
        let p = sarif(text).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].rule.as_deref(), Some("no-eval"));
        assert_eq!(p[0].level, Level::Error);
        assert_eq!(p[0].path.as_deref(), Some("src/a.js"));
        assert_eq!(p[0].line, Some(4));
        assert_eq!(p[1].level, Level::Warning);
        assert_eq!(p[1].path, None);
        assert!(sarif("[]").is_none());
        assert!(sarif("plain text").is_none());
    }

    #[test]
    fn eslint_json_messages_become_findings() {
        let text = "\n> lint\n> eslint -f json .\n\n[{\"filePath\":\"/r/a.js\",\"messages\":[\
            {\"ruleId\":\"semi\",\"severity\":2,\"message\":\"Missing semicolon.\",\"line\":3},\
            {\"ruleId\":null,\"severity\":1,\"message\":\"Parsing\",\"line\":9}]}]\n";
        let p = eslint_json(text).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].rule.as_deref(), Some("semi"));
        assert_eq!(p[0].level, Level::Error);
        assert_eq!(p[0].path.as_deref(), Some("/r/a.js"));
        assert_eq!(p[0].line, Some(3));
        assert_eq!(p[1].rule, None);
        assert_eq!(p[1].level, Level::Warning);
        assert!(eslint_json(r#"{"runs":[]}"#).is_none());
    }

    #[test]
    fn gcc_style_lines_from_common_tools() {
        let text = "\
a.c:3:5: error: unused variable 'x' [-Werror=unused-variable]
a.c:3:5: note: declared here
b.py:7: error: Name \"y\" is not defined  [name-defined]
c.py:1:8: F401 [*] `os` imported but unused
./d.go:12:2: printf: bad verb
e.c:9: warning: implicit declaration
f.erl:801: Warning: 'catch ...' is deprecated
Found 3 errors in 2 files
Time: 12:30: done
http://example.com:80: nope
";
        let p = gcc(text);
        assert_eq!(p.len(), 6, "{p:#?}");
        assert_eq!(p[0].rule.as_deref(), Some("-Werror=unused-variable"));
        assert_eq!(p[0].message, "unused variable 'x'");
        assert_eq!(p[1].rule.as_deref(), Some("name-defined"));
        assert_eq!(p[1].line, Some(7));
        assert_eq!(p[2].rule.as_deref(), Some("F401"));
        assert_eq!(p[2].message, "`os` imported but unused");
        assert_eq!(p[3].path.as_deref(), Some("./d.go"));
        assert_eq!(p[3].rule, None);
        assert_eq!(p[3].level, Level::Error);
        assert_eq!(p[4].level, Level::Warning);
        assert_eq!(p[5].level, Level::Warning);
        assert_eq!(p[5].message, "'catch ...' is deprecated");
    }

    #[test]
    fn auto_tries_json_then_lines_and_text_never_parses() {
        let sarif_text = r#"{"runs":[{"results":[{"ruleId":"r","message":{"text":"m"}}]}]}"#;
        assert_eq!(
            parse(ToolFormat::Auto, sarif_text, "")[0].rule.as_deref(),
            Some("r")
        );
        assert_eq!(parse(ToolFormat::Auto, "", "x.c:1: error: boom").len(), 1);
        assert!(parse(ToolFormat::Text, "", "x.c:1: error: boom").is_empty());
        assert!(parse(ToolFormat::Sarif, "", "x.c:1: error: boom").is_empty());
        assert!(parse(ToolFormat::Auto, "all good\n", "").is_empty());
    }
}
