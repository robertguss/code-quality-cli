use qgate::classify::{Role, classify};

#[test]
fn test_directories_and_names_are_tests() {
    for path in [
        "tests/cli_test.rs",
        "src/foo/tests/bar.rs",
        "web/__tests__/a.ts",
        "pkg/server_test.go",
        "test_api.py",
        "api_test.py",
        "conftest.py",
        "test/x_test.exs",
        "a.test.ts",
        "a.spec.tsx",
        "spec/models/user_spec.rb",
        "internal/testdata/input.go",
    ] {
        assert_eq!(classify(path), Role::Test, "{path}");
    }
}

#[test]
fn benches_and_examples_are_dev() {
    assert_eq!(classify("benches/b.rs"), Role::Dev);
    assert_eq!(classify("examples/demo.rs"), Role::Dev);
}

#[test]
fn the_word_test_alone_is_not_a_test() {
    for path in [
        "src/main.rs",
        "src/testing_utils.rs",
        "lib/contest/app.ex",
        "src/latest.py",
    ] {
        assert_eq!(classify(path), Role::Production, "{path}");
    }
}
