//! TypeScript, TSX and JavaScript escape hatches, swallowed errors and debug
//! output (U6 to U8).

mod support;

use support::rules::{count, occurrences, occurrences_with};

fn casts(n: usize) -> String {
    (0..n).map(|i| format!("const v{i} = x as T;\n")).collect()
}

#[test]
fn as_casts_count_and_as_const_does_not() {
    let rule = "escape-hatch/as-cast";
    assert_eq!(count("src/a.ts", &casts(3), rule), 3);
    let src = format!("{}const k = [1] as const;\n", casts(5));
    assert_eq!(count("src/a.ts", &src, rule), 5);
    assert_eq!(count("tests/a.test.ts", &casts(1), rule), 1, "everywhere");
}

#[test]
fn each_typescript_escape_hatch_counts_once_under_its_own_id() {
    let src = "const a = x!;\nlet b: any = 1;\nconst c = <User>data;\nfunction f(g: Function) {}\n";
    assert_eq!(
        occurrences("src/a.ts", src),
        vec![
            ("escape-hatch/non-null".into(), 1),
            ("escape-hatch/any".into(), 2),
            ("escape-hatch/angle-cast".into(), 3),
            ("escape-hatch/function-object-type".into(), 4),
        ]
    );
}

#[test]
fn escape_hatch_words_in_strings_comments_and_names_do_not_count() {
    let src = "const s = \"any x as T Function\";\n// let a: any = x!;\n/* <User>data */\nconst any = 1;\nconst n = x != y;\ninterface Function {}\nlet t: unknown = 1;\n";
    assert!(occurrences("src/a.ts", src).is_empty());
}

#[test]
fn tsx_queries_compile_and_count_without_angle_casts() {
    let src = "const a = x as T;\nconst b = y!;\nlet c: any = 1;\nconst el = <div>hi</div>;\n";
    assert_eq!(
        occurrences("src/a.tsx", src),
        vec![
            ("escape-hatch/as-cast".into(), 1),
            ("escape-hatch/non-null".into(), 2),
            ("escape-hatch/any".into(), 3),
        ]
    );
}

#[test]
fn javascript_has_no_escape_hatches_but_counts_console_log_and_empty_catch() {
    let src = "const any = 1;\nconst n = !x;\ntry { f() } catch {}\nconsole.log(any);\n";
    assert_eq!(
        occurrences("src/a.js", src),
        vec![
            ("swallowed-error/empty-catch".into(), 3),
            ("debug-output/console-log".into(), 4),
        ]
    );
}

#[test]
fn empty_and_comment_only_catches_count_and_rethrows_do_not() {
    let rule = "swallowed-error/empty-catch";
    let src = "try { f() } catch {}\ntry { f() } catch (e) { /* ignore */ }\ntry { f() } catch (e) {\n  // later\n}\n";
    assert_eq!(count("src/a.ts", src, rule), 3);
    let handled = "try { f() } catch (e) { throw e; }\ntry { f() } catch (e) { retry(); }\n";
    assert!(occurrences("src/a.ts", handled).is_empty());
}

#[test]
fn log_only_catch_is_heuristic_and_logging_then_rethrowing_is_not() {
    let rule = "swallowed-error/log-only-catch";
    let src = "try { f() } catch (e) { console.error(e) }\ntry { f() } catch (e) {\n  logger.warn(e);\n  // keep going\n}\n";
    assert_eq!(
        occurrences("src/a.ts", src),
        vec![(rule.into(), 1), (rule.into(), 2)],
        "console.error is not console-log"
    );
    let rethrow = "try { f() } catch (e) { console.error(e); throw e; }\n";
    assert!(occurrences("src/a.ts", rethrow).is_empty());
    assert_eq!(count("src/a.js", src, rule), 2);
    let r = qgate::rules::find(rule).unwrap();
    assert_eq!(r.kind, qgate::rules::Kind::Heuristic);
}

#[test]
fn console_log_counts_in_sources_but_not_entry_points_scripts_or_tests() {
    let rule = "debug-output/console-log";
    let src = "console.log(1);\nconsole.debug(2);\nconsole.info(3);\nlog(4);\n";
    assert_eq!(count("src/a.ts", src, rule), 2);
    for path in ["bin/cli.ts", "scripts/x.ts", "src/a.test.ts"] {
        assert_eq!(count(path, src, rule), 0, "{path}");
    }
}

#[test]
fn debugger_counts_in_entry_points_but_not_tests() {
    let rule = "debug-output/debugger";
    let src = "debugger;\n";
    assert_eq!(count("bin/cli.ts", src, rule), 1);
    assert_eq!(count("src/a.js", src, rule), 1);
    assert_eq!(count("src/a.test.ts", src, rule), 0);
}

#[test]
fn allow_globs_exempt_console_log_but_not_debugger() {
    let src = "console.log(1);\ndebugger;\n";
    let cfg = "[rules.debug-output]\nallow = [\"src/ui/**\"]\n";
    assert_eq!(
        occurrences_with("src/ui/a.ts", src, cfg),
        vec![("debug-output/debugger".into(), 2)]
    );
}

#[test]
fn f1_cast_and_console_log_each_count_once() {
    let src = "const user = data as User;\nconsole.log(user);\n";
    assert_eq!(
        occurrences("src/a.ts", src),
        vec![
            ("escape-hatch/as-cast".into(), 1),
            ("debug-output/console-log".into(), 2),
        ]
    );
}
