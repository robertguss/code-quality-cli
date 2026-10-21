//! Function and file length, measured on staged content.

use std::collections::HashMap;

use super::{FILE_LENGTH, FUNCTION_LENGTH, PARSE_ERROR};
use crate::cli::Finding;
use crate::config::Limits;
use crate::git::index::is_binary;
use crate::grammars::{self, Lang};

/// One measured function or file, whether or not it is over its limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Measurement {
    pub rule: &'static str,
    pub path: String,
    /// Qualified function name plus `#ordinal` among same-named functions in the file.
    pub symbol: Option<String>,
    pub line: u32,
    pub value: u32,
    pub limit: u32,
    pub body_hash: Option<u64>,
}

impl Measurement {
    pub fn over_limit(&self) -> bool {
        self.value > self.limit
    }
}

#[derive(Debug, Default)]
pub struct FileLengths {
    pub measurements: Vec<Measurement>,
    pub warnings: Vec<Finding>,
    /// The parser recovered from single-line syntax errors; lengths are intact.
    pub recovered_errors: bool,
}

/// Physical lines, counting a final line without a newline; CRLF counts the same as LF.
pub fn count_lines(text: &str) -> u32 {
    text.lines().count() as u32
}

/// Measure one file. Binary and generated content yield nothing; files without a bundled
/// grammar get only the file-length measurement.
pub fn measure(path: &str, content: &[u8], limits: Limits) -> FileLengths {
    measure_with_macros(path, content, limits, &[])
}

/// [`measure`], first blanking `attribute_macros` in C and C++ files.
pub fn measure_with_macros(
    path: &str,
    content: &[u8],
    limits: Limits,
    macros: &[String],
) -> FileLengths {
    let mut out = FileLengths::default();
    if is_binary(content) {
        return out;
    }
    let text = String::from_utf8_lossy(content);
    if super::exclude::is_generated(path, &text) {
        return out;
    }
    out.measurements.push(Measurement {
        rule: FILE_LENGTH,
        path: path.to_string(),
        symbol: None,
        line: 1,
        value: count_lines(&text),
        limit: limits.file_lines,
        body_hash: None,
    });
    let Some(lang) = Lang::detect(path) else {
        return out;
    };
    let text = match lang {
        Lang::C | Lang::Cpp if !macros.is_empty() => grammars::blank_macros(&text, macros).into(),
        _ => text,
    };
    let Some(parsed) = grammars::parse(lang, &text) else {
        out.warnings.push(
            Finding::warning(PARSE_ERROR, format!("could not parse as {}", lang.name()))
                .at(path, None),
        );
        return out;
    };
    if parsed.multi_line_error {
        out.warnings.push(
            Finding::warning(
                PARSE_ERROR,
                format!(
                    "syntax errors spanning several lines while parsing as {}; function lengths may be off or functions missed",
                    lang.name()
                ),
            )
            .at(path, None),
        );
    } else if parsed.has_errors {
        out.recovered_errors = true;
    }
    let mut seen: HashMap<String, u32> = HashMap::new();
    for f in parsed.functions {
        let ordinal = seen.entry(f.name.clone()).or_default();
        out.measurements.push(Measurement {
            rule: FUNCTION_LENGTH,
            path: path.to_string(),
            symbol: Some(format!("{}#{ordinal}", f.name)),
            line: f.start_line,
            value: f.lines(),
            limit: limits.function_lines,
            body_hash: Some(f.body_hash),
        });
        *ordinal += 1;
    }
    out
}
