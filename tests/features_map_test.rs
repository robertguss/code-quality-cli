use qgate::cli::Severity;
use qgate::config::Tasks;
use qgate::features::map::FeatureMap;

fn map(text: &str) -> FeatureMap {
    FeatureMap::parse(text).unwrap()
}

fn problems(text: &str) -> Vec<String> {
    map(text)
        .validate(&Tasks::default().regex())
        .into_iter()
        .inspect(|f| {
            assert_eq!(f.rule, "feature-map");
            assert_eq!(f.severity, Severity::Error);
            assert_eq!(f.path.as_deref(), Some("qgate.features.toml"));
        })
        .map(|f| f.message)
        .collect()
}

const NESTED: &str = r#"
[[feature]]
name = "outer"
purpose = "Outer"
task = "QG-1"
folders = ["src/a"]

[[feature]]
name = "inner"
purpose = "Inner"
task = "QG-2"
folders = ["src/a/b/"]
entry_points = ["src/c/main.rs"]
"#;

#[test]
fn the_nearest_owning_folder_wins() {
    let m = map(NESTED);
    let owner = |p: &str| m.owner(p).map(|f| f.name.as_str());
    assert_eq!(owner("src/a/b/x.rs"), Some("inner"));
    assert_eq!(owner("src/a/b"), Some("inner"));
    assert_eq!(owner("src/a/y.rs"), Some("outer"));
    assert_eq!(owner("src/ab.rs"), None);
    assert!(problems(NESTED).is_empty());
}

#[test]
fn a_listed_file_belongs_to_its_feature_inside_another_features_folder() {
    let m = map(&format!(
        "{NESTED}\n[[feature]]\nname = \"tool\"\npurpose = \"T\"\ntask = \"QG-3\"\nentry_points = [\"src/a/tool.rs\"]\n"
    ));
    assert_eq!(m.owner("src/a/tool.rs").unwrap().name, "tool");
    assert_eq!(m.owner("src/c/main.rs").unwrap().name, "inner");
}

#[test]
fn a_folder_owned_twice_is_one_error_naming_both() {
    let p = problems(
        "[[feature]]\nname = \"one\"\npurpose = \"P\"\nfolders = [\"src/a\"]\n\
         [[feature]]\nname = \"two\"\npurpose = \"P\"\nfolders = [\"src/a/\"]\n",
    );
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(
        p[0].contains("`src/a`") && p[0].contains("`one`") && p[0].contains("`two`"),
        "{p:?}"
    );
}

#[test]
fn named_features_need_a_purpose_and_placeholders_need_nothing() {
    let p = problems("[[feature]]\nname = \"a\"\nfolders = [\"src/a\"]\n");
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("`a` has no `purpose`"), "{p:?}");
    assert!(
        problems("[[feature]]\nname = \"a\"\npurpose = \"P\"\nfolders = [\"src/a\"]\n").is_empty()
    );
    assert!(
        problems("[[feature]]\nname = \"a\"\nplaceholder = true\nfolders = [\"src/a\"]\n")
            .is_empty()
    );
}

#[test]
fn duplicate_names_and_empty_features_are_errors() {
    let p = problems(
        "[[feature]]\nname = \"a\"\nplaceholder = true\nfolders = [\"x\"]\n\
         [[feature]]\nname = \"a\"\nplaceholder = true\nfolders = [\"y\"]\n\
         [[feature]]\nname = \"empty\"\nplaceholder = true\n",
    );
    assert_eq!(p.len(), 2, "{p:?}");
    assert!(
        p.iter().any(|m| m.contains("`a` is declared twice")),
        "{p:?}"
    );
    assert!(
        p.iter().any(|m| m.contains("`empty` owns no folders")),
        "{p:?}"
    );
}

#[test]
fn a_task_must_match_the_pattern() {
    let text = "[[feature]]\nname = \"a\"\npurpose = \"P\"\ntask = \"qg-4\"\nfolders = [\"a\"]\n";
    let p = problems(text);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("`qg-4`"), "{p:?}");
    let lower = Tasks {
        pattern: "[a-z]+-[0-9]+".into(),
    };
    assert!(map(text).validate(&lower.regex()).is_empty());
}

#[test]
fn unknown_keys_and_bad_toml_are_named() {
    let e = FeatureMap::parse("[[feature]]\nname = \"a\"\nfolder = [\"a\"]\n").unwrap_err();
    assert!(e.contains("folder"), "{e}");
    assert!(FeatureMap::parse("[[feature]\n").is_err());
    assert!(map("").features.is_empty());
}

#[test]
fn an_empty_folder_owning_the_whole_repo_is_an_error() {
    let p = problems(
        "[[feature]]\nname = \"all\"\npurpose = \"All\"\ntask = \"QG-1\"\nfolders = [\"/\"]\n",
    );
    assert!(p.iter().any(|m| m.contains("empty path")), "{p:?}");
}

#[test]
fn an_entry_point_listed_by_two_features_is_an_error() {
    let p = problems(
        "[[feature]]\nname = \"a\"\npurpose = \"A\"\nentry_points = [\"src/x.rs\"]\n\n\
         [[feature]]\nname = \"b\"\npurpose = \"B\"\nentry_points = [\"src/x.rs\"]\n",
    );
    assert!(
        p.iter()
            .any(|m| m.contains("`src/x.rs` is owned by `a` and `b`")),
        "{p:?}"
    );
}
