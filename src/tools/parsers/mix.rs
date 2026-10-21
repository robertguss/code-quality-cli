//! Mix and ExUnit output: compile diagnostics, test failures and
//! `mix format --check-formatted`. Credo needs no parser of its own:
//! `mix credo --format sarif` goes through the SARIF parser.

use super::{Level, Parsed, all_digits};

/// Printed by `mix compile --warnings-as-errors` when warnings failed the build.
const WARNINGS_FAILED: &str = "Compilation failed due to warnings";
const FORMAT_HEADER: &str = "The following files are not formatted:";
/// How far below a `warning:` or `error:` line its location may be.
const DIAGNOSTIC_SPAN: usize = 30;

/// Every Mix, ExUnit and formatter finding in `text` (ANSI already stripped).
/// Empty when the text has none of their shapes.
pub fn parse(text: &str) -> Vec<Parsed> {
    let lines: Vec<&str> = text.lines().collect();
    let warning_level = match text.contains(WARNINGS_FAILED) {
        true => Level::Error,
        false => Level::Warning,
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if let Some((level, message)) = diagnostic(line, warning_level)
            && let Some((path, line_no)) = diagnostic_location(&lines[i + 1..])
        {
            out.push(Parsed {
                rule: None,
                level,
                path: Some(path),
                line: Some(line_no),
                message: message.to_string(),
            });
        } else if let Some(name) = test_failure(lines[i]) {
            out.push(exunit(name, &lines[i + 1..]));
        } else if line == FORMAT_HEADER {
            out.extend(unformatted(&lines[i + 1..]));
        }
        i += 1;
    }
    out
}

fn diagnostic(line: &str, warning_level: Level) -> Option<(Level, &str)> {
    if let Some(m) = line.strip_prefix("warning: ") {
        return Some((warning_level, m.trim()));
    }
    line.strip_prefix("error: ")
        .map(|m| (Level::Error, m.trim()))
}

/// The `└─ path:line[:col][: Module.fun/1]` line of an Elixir 1.15+
/// diagnostic, or the indented `path:line: Module.fun/1` line before 1.15.
fn diagnostic_location(rest: &[&str]) -> Option<(String, u32)> {
    for (n, line) in rest.iter().take(DIAGNOSTIC_SPAN).enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(loc) = trimmed.strip_prefix("└─ ") {
            return path_line(loc);
        }
        // Before 1.15 the location is the next line, indented.
        if n == 0 && line.starts_with(' ') && !trimmed.starts_with('│') {
            return path_line(trimmed);
        }
    }
    None
}

/// `path:line`, optionally followed by `:col` and `: context`.
fn path_line(s: &str) -> Option<(String, u32)> {
    let (path, rest) = s.split_once(':')?;
    if path.is_empty() || path.contains(char::is_whitespace) {
        return None;
    }
    let num = rest.split(':').next()?;
    Some((path.to_string(), all_digits(num)?.parse().ok()?))
}

/// The name in an ExUnit failure header such as `  1) test adds (MathTest)`.
fn test_failure(line: &str) -> Option<&str> {
    if !line.starts_with("  ") {
        return None;
    }
    let (num, name) = line.trim().split_once(") ")?;
    all_digits(num)?;
    let is_failure = ["test ", "doctest ", "property "]
        .iter()
        .any(|p| name.starts_with(p))
        || name.contains(": failure on setup_all callback");
    is_failure.then_some(name.trim())
}

/// A failure at the `test/x_test.exs:12` line under its header, with the
/// first line of the explanation as the message.
fn exunit(name: &str, rest: &[&str]) -> Parsed {
    let body: Vec<&str> = rest
        .iter()
        .map(|l| l.trim())
        .take_while(|l| !l.is_empty())
        .collect();
    let location = body.first().and_then(|l| {
        let (path, num) = l.rsplit_once(':')?;
        let line_no = all_digits(num)?.parse().ok()?;
        (!path.contains(char::is_whitespace)).then(|| (path.to_string(), line_no))
    });
    let detail = body
        .iter()
        .skip(usize::from(location.is_some()))
        .find(|l| !l.starts_with("code:"))
        .copied();
    let message = match detail {
        Some(d) => format!("{name}: {d}"),
        None => name.to_string(),
    };
    let (path, line) = match location {
        Some((p, l)) => (Some(p), Some(l)),
        None => (None, None),
    };
    Parsed {
        rule: Some("test".to_string()),
        level: Level::Error,
        path,
        line,
        message,
    }
}

/// The file list after `mix format --check-formatted`'s header: `  * path`
/// before Elixir 1.17, a bare path above each diff since.
fn unformatted(rest: &[&str]) -> Vec<Parsed> {
    rest.iter()
        .map(|l| l.trim())
        .map(|l| l.strip_prefix("* ").unwrap_or(l))
        .filter(|l| {
            !l.is_empty() && !l.contains(char::is_whitespace) && !l.contains('|') && l.contains('.')
        })
        .map(|path| Parsed {
            rule: Some("format".to_string()),
            level: Level::Error,
            path: Some(path.to_string()),
            line: None,
            message: "not formatted; run `mix format`".to_string(),
        })
        .collect()
}
