//! `qgate.baseline`: sorted lines, one entry each, so diffs stay small and
//! merges rarely conflict. Fields are tab-separated (shown here as spaces).
//!
//! ```text
//! function-length  src/app.rs  Server::run#0  80
//! file-length  src/app.rs  -  612
//! duplication  -  9f2c41d07a3b55e1  3x64
//! ```

use std::collections::BTreeMap;
use std::fmt;

use anyhow::{Result, bail};

use crate::checks::{DUPLICATION, FILE_LENGTH, FUNCTION_LENGTH};

const HEADER: &str = "# qgate baseline v1: existing violations. Growth is reported as a loosening; shrink with `qgate baseline prune`.";

/// Rule, path (`-` for duplication) and symbol (`-` for a file, the block hash for duplication).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    pub rule: String,
    pub path: String,
    pub symbol: String,
}

impl Key {
    pub fn length(rule: &str, path: &str, symbol: Option<&str>) -> Key {
        Key {
            rule: rule.to_string(),
            path: path.to_string(),
            symbol: symbol.unwrap_or("-").to_string(),
        }
    }

    pub fn clones(block: &str) -> Key {
        Key {
            rule: DUPLICATION.to_string(),
            path: "-".to_string(),
            symbol: block.to_string(),
        }
    }

    /// `path` (`symbol`) for messages.
    pub fn describe(&self) -> String {
        match self.rule.as_str() {
            DUPLICATION => format!("duplicated block {}", self.symbol),
            FILE_LENGTH => format!("file {}", self.path),
            _ => format!("{} in {}", self.symbol, self.path),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Lines(u32),
    Clones { members: u32, tokens: u32 },
}

impl Value {
    /// The lower of two values, component by component.
    pub fn min(self, other: Value) -> Value {
        match (self, other) {
            (Value::Lines(a), Value::Lines(b)) => Value::Lines(a.min(b)),
            (
                Value::Clones {
                    members: a,
                    tokens: t,
                },
                Value::Clones {
                    members: b,
                    tokens: u,
                },
            ) => Value::Clones {
                members: a.min(b),
                tokens: t.min(u),
            },
            (a, _) => a,
        }
    }

    /// True when `self` exceeds `limit` in any component.
    pub fn exceeds(self, limit: Value) -> bool {
        match (self, limit) {
            (Value::Lines(a), Value::Lines(b)) => a > b,
            (
                Value::Clones {
                    members: a,
                    tokens: t,
                },
                Value::Clones {
                    members: b,
                    tokens: u,
                },
            ) => a > b || t > u,
            _ => true,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Lines(n) => write!(f, "{n}"),
            Value::Clones { members, tokens } => write!(f, "{members}x{tokens}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Baseline {
    pub entries: BTreeMap<Key, Value>,
}

impl Baseline {
    pub fn parse(text: &str) -> Result<Baseline> {
        let mut entries = BTreeMap::new();
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let [rule, path, symbol, value] = fields[..] else {
                bail!("qgate.baseline:{}: expected 4 tab-separated fields", n + 1);
            };
            let parsed = match rule {
                FUNCTION_LENGTH | FILE_LENGTH => value.parse().ok().map(Value::Lines),
                DUPLICATION => value.split_once('x').and_then(|(m, t)| {
                    Some(Value::Clones {
                        members: m.parse().ok()?,
                        tokens: t.parse().ok()?,
                    })
                }),
                _ => bail!("qgate.baseline:{}: unknown rule `{rule}`", n + 1),
            };
            let Some(value) = parsed else {
                bail!("qgate.baseline:{}: bad value `{value}`", n + 1);
            };
            entries.insert(
                Key {
                    rule: rule.to_string(),
                    path: path.to_string(),
                    symbol: symbol.to_string(),
                },
                value,
            );
        }
        Ok(Baseline { entries })
    }

    pub fn render(&self) -> String {
        let mut out = String::from(HEADER);
        out.push('\n');
        for (k, v) in &self.entries {
            out.push_str(&format!("{}\t{}\t{}\t{v}\n", k.rule, k.path, k.symbol));
        }
        out
    }

    pub fn get(&self, key: &Key) -> Option<Value> {
        self.entries.get(key).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_sorted() {
        let text = format!(
            "{HEADER}\nduplication\t-\tabc\t3x64\nfile-length\ta.rs\t-\t600\nfunction-length\ta.rs\tf#0\t80\n"
        );
        let b = Baseline::parse(&text).unwrap();
        assert_eq!(b.entries.len(), 3);
        assert_eq!(b.render(), text);
    }

    #[test]
    fn rejects_bad_lines() {
        assert!(Baseline::parse("function-length\ta.rs\t80\n").is_err());
        assert!(Baseline::parse("function-length\ta.rs\tf#0\tlots\n").is_err());
        assert!(Baseline::parse("speed\ta.rs\tf#0\t1\n").is_err());
    }
}
