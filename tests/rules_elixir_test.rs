//! Elixir escape hatches, swallowed errors and debug output (U6 to U8).

mod support;

use support::rules::{count, occurrences};

const NO_SPEC: &str = "escape-hatch/ex-no-spec";

fn module(body: &str) -> String {
    format!("defmodule App do\n{body}\nend\n")
}

#[test]
fn public_def_without_spec_counts_and_defp_does_not() {
    let src = module(
        "  def f(x), do: x\n  defp g(x), do: x\n  defmacro m(x), do: x\n  def h do\n    1\n  end",
    );
    assert_eq!(
        occurrences("lib/app.ex", &src),
        vec![(NO_SPEC.into(), 2), (NO_SPEC.into(), 5)]
    );
}

#[test]
fn multi_clause_def_with_one_spec_counts_zero_times() {
    let src = module(
        "  @spec f(integer) :: integer\n  def f(0), do: 0\n  def f(n) when n > 0, do: n\n  def f(n), do: -n",
    );
    assert_eq!(count("lib/app.ex", &src, NO_SPEC), 0);
}

#[test]
fn multi_clause_def_without_spec_counts_once() {
    let src = module("  def f(0), do: 0\n  def f(n), do: n");
    assert_eq!(occurrences("lib/app.ex", &src), vec![(NO_SPEC.into(), 2)]);
}

#[test]
fn spec_is_matched_by_name_and_arity() {
    let src = module(
        "  @spec f(integer) :: integer\n  def f(x, y), do: x + y\n  @spec g() :: :ok\n  def g, do: :ok\n  @spec k(t) :: t when t: term\n  def k(x), do: x",
    );
    assert_eq!(occurrences("lib/app.ex", &src), vec![(NO_SPEC.into(), 3)]);
}

#[test]
fn attributes_between_spec_and_def_still_count_as_specced() {
    let src = module(
        "  @spec f(integer) :: integer\n  @doc \"Doubles.\"\n  @impl false\n  def f(x), do: x * 2",
    );
    assert_eq!(count("lib/app.ex", &src, NO_SPEC), 0);
}

#[test]
fn spec_earlier_in_the_module_counts() {
    let src = module("  @spec f(integer) :: integer\n  def g, do: 1\n  def f(x), do: x");
    assert_eq!(occurrences("lib/app.ex", &src), vec![(NO_SPEC.into(), 3)]);
}

#[test]
fn spec_in_another_module_does_not_count() {
    let src = "defmodule A do\n  @spec f(integer) :: integer\n  def f(x), do: x\nend\ndefmodule B do\n  def f(x), do: x\nend\n";
    assert_eq!(occurrences("lib/app.ex", src), vec![(NO_SPEC.into(), 6)]);
}

#[test]
fn callbacks_and_protocol_defs_do_not_count() {
    let src = "defmodule A do\n  @impl true\n  def init(x), do: {:ok, x}\n  @impl GenServer\n  def handle_call(_, _, s), do: {:reply, :ok, s}\nend\ndefprotocol P do\n  def size(x)\nend\ndefimpl P, for: Map do\n  def size(m), do: map_size(m)\nend\n";
    assert_eq!(count("lib/app.ex", src, NO_SPEC), 0);
}

#[test]
fn to_atom_counts_only_with_a_non_literal_argument() {
    let src = module(
        "  @spec f(String.t()) :: atom\n  def f(name) do\n    String.to_atom(name)\n    String.to_atom(\"lit\")\n    String.to_existing_atom(name)\n    name |> String.to_atom()\n  end",
    );
    assert_eq!(
        occurrences("lib/app.ex", &src),
        vec![
            ("escape-hatch/ex-to-atom".into(), 4),
            ("escape-hatch/ex-to-atom".into(), 7),
        ]
    );
}

#[test]
fn code_eval_counts_everywhere() {
    let src = module(
        "  @spec f(String.t()) :: term\n  def f(s) do\n    Code.eval_string(s)\n    Code.eval_quoted(q)\n    Code.string_to_quoted(s)\n  end",
    );
    assert_eq!(
        occurrences("lib/app.ex", &src),
        vec![
            ("escape-hatch/ex-eval".into(), 4),
            ("escape-hatch/ex-eval".into(), 5),
        ]
    );
    assert_eq!(count("test/a_test.exs", &src, "escape-hatch/ex-eval"), 2);
}

#[test]
fn rescue_returning_a_literal_counts() {
    let src = module(
        "  @spec f() :: term\n  def f do\n    g()\n  rescue\n    _ -> nil\n  end\n  @spec h() :: term\n  def h do\n    g()\n  rescue\n    _e in ArgumentError -> :error\n  catch\n    _, _ -> :ok\n  end",
    );
    assert_eq!(
        occurrences("lib/app.ex", &src),
        vec![
            ("swallowed-error/ex-rescue-nil".into(), 6),
            ("swallowed-error/ex-rescue-nil".into(), 12),
            ("swallowed-error/ex-rescue-nil".into(), 14),
        ]
    );
}

#[test]
fn rescue_that_logs_and_reraises_does_not_count() {
    let src = module(
        "  @spec f() :: term\n  def f do\n    g()\n  rescue\n    e ->\n      Logger.error(e)\n      reraise e, __STACKTRACE__\n  catch\n    :exit, _ -> {:error, :exit}\n  end\n  @spec h() :: term\n  def h do\n    g()\n  rescue\n    e -> nil\n  end",
    );
    assert!(occurrences("lib/app.ex", &src).is_empty());
}

#[test]
fn puts_counts_only_under_lib_outside_tasks_and_tests() {
    let src = "IO.puts(\"hi\")\n";
    let rule = "debug-output/ex-puts";
    assert_eq!(count("lib/app.ex", src, rule), 1);
    assert_eq!(count("apps/web/lib/web.ex", src, rule), 1);
    for path in [
        "lib/mix/tasks/seed.ex",
        "test/a_test.exs",
        "config/runtime.exs",
        "mix.exs",
    ] {
        assert_eq!(count(path, src, rule), 0, "{path}");
    }
}

#[test]
fn inspect_counts_in_tasks_but_not_tests() {
    let src = "IO.inspect(x)\nx |> IO.inspect(label: \"x\")\n";
    let rule = "debug-output/ex-inspect";
    assert_eq!(count("lib/app.ex", src, rule), 2);
    assert_eq!(count("lib/mix/tasks/seed.ex", src, rule), 2);
    assert_eq!(count("test/a_test.exs", src, rule), 0);
}

#[test]
fn calls_inside_strings_and_comments_do_not_count() {
    let src = "# IO.inspect(x)\ns = \"IO.puts(1) Code.eval_string(s)\"\n";
    assert!(occurrences("lib/app.ex", src).is_empty());
}
