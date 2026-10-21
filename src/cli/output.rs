//! One output type for every command: rendered as short text by default or as
//! a single JSON envelope with `--format json`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    #[default]
    Text,
    Json,
    /// GitHub Actions workflow commands (annotations), then the text report.
    Github,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Fails the run.
    Error,
    /// Reported, does not fail the run.
    Warning,
    /// Something the caller should do, such as pruning the baseline.
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

/// One problem found by a check, tied to a file and line where possible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub rule: String,
    pub severity: Severity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub message: String,
    /// The exact command that resolves this finding, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Finding {
    pub fn error(rule: &str, message: impl Into<String>) -> Self {
        Finding {
            rule: rule.to_string(),
            severity: Severity::Error,
            path: None,
            line: None,
            message: message.into(),
            fix: None,
        }
    }

    pub fn warning(rule: &str, message: impl Into<String>) -> Self {
        Finding {
            severity: Severity::Warning,
            ..Finding::error(rule, message)
        }
    }

    pub fn info(rule: &str, message: impl Into<String>) -> Self {
        Finding {
            severity: Severity::Info,
            ..Finding::error(rule, message)
        }
    }

    pub fn at(mut self, path: impl Into<String>, line: Option<u32>) -> Self {
        self.path = Some(path.into());
        self.line = line;
        self
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    fn location(&self) -> String {
        match (&self.path, self.line) {
            (Some(p), Some(l)) => format!("{p}:{l}: "),
            (Some(p), None) => format!("{p}: "),
            _ => String::new(),
        }
    }
}

/// The result of one command.
#[derive(Debug, Serialize)]
pub struct Report {
    pub command: String,
    pub passed: bool,
    pub findings: Vec<Finding>,
    /// Distinct fix commands, in the order their findings appear.
    pub fixes: Vec<String>,
    /// Wall time per step, in milliseconds.
    pub timings_ms: BTreeMap<String, u64>,
    /// Machine-useful facts about what the command did, such as files written.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub data: BTreeMap<String, serde_json::Value>,
}

impl Report {
    pub fn new(command: &str) -> Self {
        Report {
            command: command.to_string(),
            passed: true,
            findings: Vec::new(),
            fixes: Vec::new(),
            timings_ms: BTreeMap::new(),
            data: BTreeMap::new(),
        }
    }

    pub fn timing(&mut self, step: &str, elapsed: Duration) {
        self.timings_ms
            .insert(step.to_string(), elapsed.as_millis() as u64);
    }

    pub fn set(&mut self, key: &str, value: impl Into<serde_json::Value>) {
        self.data.insert(key.to_string(), value.into());
    }

    pub fn extend(&mut self, findings: impl IntoIterator<Item = Finding>) {
        self.findings.extend(findings);
    }

    /// Sort findings, collect fix commands and decide pass or fail.
    pub fn finish(mut self) -> Self {
        self.findings.sort_by(|a, b| {
            (a.severity, &a.path, a.line, &a.rule, &a.message)
                .cmp(&(b.severity, &b.path, b.line, &b.rule, &b.message))
        });
        self.findings.dedup();
        self.passed = !self.findings.iter().any(|f| f.severity == Severity::Error);
        self.fixes.clear();
        for fix in self.findings.iter().filter_map(|f| f.fix.as_ref()) {
            if !self.fixes.contains(fix) {
                self.fixes.push(fix.clone());
            }
        }
        self
    }

    pub fn exit_code(&self) -> i32 {
        if self.passed { 0 } else { 1 }
    }

    pub fn render(&self, format: Format) -> String {
        match format {
            Format::Json => serde_json::to_string_pretty(self).expect("report serializes") + "\n",
            Format::Text => self.render_text(),
            Format::Github => self.render_github() + &self.render_text(),
        }
    }

    /// One `::error`/`::warning`/`::notice` workflow command per finding.
    fn render_github(&self) -> String {
        let mut out = String::new();
        for f in &self.findings {
            let level = match f.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "notice",
            };
            let mut props = Vec::new();
            if let Some(p) = &f.path {
                props.push(format!("file={}", escape_property(p)));
                if let Some(l) = f.line {
                    props.push(format!("line={l}"));
                }
            }
            props.push(format!(
                "title={}",
                escape_property(&format!("qgate {}", f.rule))
            ));
            let mut message = f.message.clone();
            if let Some(fix) = &f.fix {
                message = format!("{message}\nfix: {fix}");
            }
            let _ = writeln!(
                out,
                "::{level} {}::{}",
                props.join(","),
                escape_data(&message)
            );
        }
        out
    }

    /// A Markdown summary for `$GITHUB_STEP_SUMMARY`.
    pub fn github_summary(&self) -> String {
        let verdict = if self.passed { "pass" } else { "FAIL" };
        let mut out = format!("### qgate {}: {verdict}\n\n", self.command);
        if self.findings.is_empty() {
            out.push_str("No findings.\n");
            return out;
        }
        out.push_str("| Severity | Rule | Location | Message |\n| --- | --- | --- | --- |\n");
        for f in &self.findings {
            let sev = f.severity.as_str();
            let loc = f.location();
            let cell = |s: &str| s.replace('|', "\\|").replace('\n', " ");
            let _ = writeln!(
                out,
                "| {sev} | {} | {} | {} |",
                cell(&f.rule),
                cell(loc.trim_end_matches(": ")),
                cell(&f.message)
            );
        }
        out
    }

    fn render_text(&self) -> String {
        let errors = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count();
        let total_ms: u64 = self.timings_ms.get("total").copied().unwrap_or_default();
        let mut out = String::new();
        let verdict = if self.passed { "pass" } else { "FAIL" };
        let _ = write!(out, "qgate {}: {verdict}", self.command);
        if !self.findings.is_empty() {
            let _ = write!(out, " ({errors} errors, {} findings)", self.findings.len());
        }
        if total_ms > 0 {
            let _ = write!(out, " in {:.1}s", total_ms as f64 / 1000.0);
        }
        out.push('\n');
        for (key, value) in &self.data {
            let value = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let _ = writeln!(out, "{key}: {value}");
        }
        for f in &self.findings {
            let sev = f.severity.as_str();
            let _ = writeln!(out, "{}{sev}[{}] {}", f.location(), f.rule, f.message);
            if let Some(fix) = &f.fix {
                let _ = writeln!(out, "  fix: {fix}");
            }
        }
        out
    }
}

/// Escape a workflow command's message.
fn escape_data(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Escape a workflow command's property value.
fn escape_property(s: &str) -> String {
    escape_data(s).replace(':', "%3A").replace(',', "%2C")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_fail_and_fixes_are_deduplicated() {
        let mut r = Report::new("run");
        r.extend([
            Finding::error("function-length", "too long")
                .at("a.rs", Some(3))
                .with_fix("qgate approve"),
            Finding::error("file-length", "too long")
                .at("b.rs", None)
                .with_fix("qgate approve"),
            Finding::info("baseline-prunable", "shrank").with_fix("qgate baseline prune"),
        ]);
        let r = r.finish();
        assert!(!r.passed);
        assert_eq!(r.exit_code(), 1);
        assert_eq!(r.fixes, vec!["qgate approve", "qgate baseline prune"]);
        let text = r.render(Format::Text);
        assert!(text.starts_with("qgate run: FAIL (2 errors, 3 findings)"));
        assert!(text.contains("a.rs:3: error[function-length] too long\n  fix: qgate approve"));
    }

    #[test]
    fn info_only_passes() {
        let mut r = Report::new("run");
        r.extend([Finding::info("baseline-prunable", "shrank")]);
        assert!(r.finish().passed);
    }
}
