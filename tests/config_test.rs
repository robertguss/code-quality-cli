use qgate::config::{Config, Stage, VERSION};

fn parse(body: &str) -> Result<Config, qgate::cli::UsageError> {
    Config::parse(
        &format!("qgate_version = \"{VERSION}\"\n{body}"),
        "qgate.toml",
    )
}

#[test]
fn defaults_apply() {
    let c = parse("").unwrap();
    assert_eq!(c.limits.function_lines, 50);
    assert_eq!(c.limits.file_lines, 500);
    assert!(c.duplication.enabled);
    assert_eq!(c.duplication.min_tokens, 50);
}

#[test]
fn example_config_parses() {
    let text = std::fs::read_to_string("qgate.toml.example").unwrap();
    let text = text.replace(
        "qgate_version = \"0.1.0\"",
        &format!("qgate_version = \"{VERSION}\""),
    );
    let c = Config::parse(&text, "qgate.toml.example").unwrap();
    assert_eq!(c.tools.len(), 2);
    assert_eq!(c.tools[1].stage, Stage::Push);
    assert_eq!(c.limits_for("services/legacy/a.rs").function_lines, 80);
    assert_eq!(c.limits_for("services/legacy2/a.rs").function_lines, 50);
}

#[test]
fn version_mismatch_names_install_command() {
    let err = Config::parse("qgate_version = \"9.9.9\"", "qgate.toml").unwrap_err();
    assert!(err.message.contains("9.9.9"));
    let hint = err.hint.unwrap();
    assert!(
        hint.contains("cargo install") && hint.contains("v9.9.9"),
        "{hint}"
    );
}

#[test]
fn unknown_key_names_key_and_file() {
    let err = parse("[limits]\nfunction_length = 3\n").unwrap_err();
    assert!(err.message.starts_with("qgate.toml:"), "{}", err.message);
    assert!(err.message.contains("function_length"), "{}", err.message);
}

#[test]
fn duplicate_tool_names_rejected() {
    let err =
        parse("[[tool]]\nname = \"a\"\ncommand = \"x\"\n[[tool]]\nname = \"a\"\ncommand = \"y\"\n")
            .unwrap_err();
    assert!(err.message.contains("declared twice"));
}

#[test]
fn starter_round_trips() {
    let c = Config::starter();
    assert_eq!(Config::parse(&c.to_toml(), "qgate.toml").unwrap(), c);
}
