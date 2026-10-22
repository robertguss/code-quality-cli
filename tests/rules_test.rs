//! The rule registry and `[rules]` levels (U2).

use qgate::cli::{Finding, Severity};
use qgate::config::{Config, VERSION};
use qgate::rules::{self, Kind, Level, Setting, apply_levels, find, level_for};

fn parse(rules: &str) -> Result<Config, qgate::cli::UsageError> {
    Config::parse(
        &format!("qgate_version = \"{VERSION}\"\n{rules}"),
        "qgate.toml",
    )
}

#[test]
fn defaults_follow_the_kind() {
    let c = Config::starter();
    assert_eq!(
        level_for(&c, find("function-length").unwrap()),
        Level::Error
    );
    for r in rules::all() {
        let want = match r.kind {
            Kind::Exact => Level::Error,
            Kind::Heuristic => Level::Warn,
        };
        assert_eq!(level_for(&c, r), want, "{}", r.id);
    }
}

#[test]
fn registry_rows_are_complete_and_unique() {
    let mut ids: Vec<&str> = rules::all().iter().map(|r| r.id).collect();
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n, "duplicate rule ids");
    for r in rules::all() {
        assert!(!r.rationale.is_empty() && !r.fix.is_empty(), "{}", r.id);
        assert!(!r.source.is_empty() && !r.since.is_empty(), "{}", r.id);
        if r.id.contains('/') {
            assert!(r.counted, "{} is an occurrence rule", r.id);
            assert_eq!(r.since, "0.6.0", "{}", r.id);
        }
    }
}

#[test]
fn family_level_and_exact_override() {
    let c =
        parse("[rules]\n\"escape-hatch\" = \"warn\"\n\"escape-hatch/rust-unwrap\" = \"error\"\n")
            .unwrap();
    for r in rules::all().iter().filter(|r| r.family() == "escape-hatch") {
        let want = if r.id == "escape-hatch/rust-unwrap" {
            Level::Error
        } else {
            Level::Warn
        };
        assert_eq!(level_for(&c, r), want, "{}", r.id);
    }
}

#[test]
fn off_drops_findings_and_warn_downgrades_errors() {
    let c = parse("[rules]\n\"function-length\" = \"off\"\n\"file-length\" = \"warn\"\n").unwrap();
    let findings = vec![
        Finding::error("function-length", "long"),
        Finding::error("file-length", "long"),
        Finding::error("duplication", "copied"),
        Finding::warning("parse-error", "odd"),
    ];
    let out = apply_levels(&c, findings);
    let got: Vec<(&str, Severity)> = out.iter().map(|f| (f.rule.as_str(), f.severity)).collect();
    assert_eq!(
        got,
        vec![
            ("file-length", Severity::Warning),
            ("duplication", Severity::Error),
            ("parse-error", Severity::Warning),
        ]
    );
}

#[test]
fn unknown_rule_is_a_usage_error_naming_qgate_rules() {
    let e = parse("[rules]\n\"no-such-rule\" = \"warn\"\n").unwrap_err();
    assert!(e.to_string().contains("no-such-rule"), "{e}");
    assert!(format!("{e:?}").contains("qgate rules"), "{e:?}");
    assert!(parse("[rules]\n\"escape-hatch\" = \"loud\"\n").is_err());
}

#[test]
fn debug_output_table_sets_level_and_allow() {
    let c = parse("[rules.debug-output]\nlevel = \"warn\"\nallow = [\"src/cli/**\"]\n").unwrap();
    assert_eq!(c.output_globs(), ["src/cli/**".to_string()]);
    assert_eq!(c.rules["debug-output"].level(), Some(Level::Warn));
    let only_allow = parse("[rules.debug-output]\nallow = [\"bin/**\"]\n").unwrap();
    assert!(matches!(
        only_allow.rules["debug-output"],
        Setting::Table(_)
    ));
    assert_eq!(only_allow.rules["debug-output"].level(), None);
}

#[test]
fn allow_is_only_for_debug_output_and_must_be_a_glob() {
    assert!(parse("[rules.escape-hatch]\nallow = [\"src/**\"]\n").is_err());
    assert!(parse("[rules.debug-output]\nallow = [\"src/[\"]\n").is_err());
    assert!(parse("[rules.debug-output]\nlevel = \"warn\"\nbogus = 1\n").is_err());
}

#[test]
fn starter_config_round_trips_without_rule_tables() {
    let text = Config::starter().to_toml();
    assert!(!text.contains("[rules"), "{text}");
    assert!(!text.contains("[suppressions"), "{text}");
    let c = parse("[suppressions]\nmax_days = 30\n").unwrap();
    assert_eq!(c.suppressions.max_days, 30);
    assert!(
        c.suppressions
            .always_block
            .contains(&"swallowed-error".to_string())
    );
}
