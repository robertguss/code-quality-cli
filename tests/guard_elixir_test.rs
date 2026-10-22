//! The loosening guard on Elixir projects.

mod support;

use support::guard::{assert_pending, pending, setup};
use support::repo::TestRepo;

const MIX_EXS: &str = "defmodule Elara.MixProject do
  use Mix.Project

  def project do
    [app: :elara, version: \"0.1.0\", aliases: aliases(), deps: deps()]
  end

  defp deps, do: [{:jason, \"~> 1.4\"}]

  defp aliases do
    [test: [\"ecto.create --quiet\", \"test\"]]
  end
end
";

/// A committed Mix project.
fn mix_setup() -> TestRepo {
    let t = setup();
    t.write("mix.exs", MIX_EXS)
        .write(".formatter.exs", "[inputs: [\"{lib,test}/**/*.{ex,exs}\"]]\n")
        .write("test/test_helper.exs", "ExUnit.start()\n")
        .write(
            "test/math_test.exs",
            "defmodule MathTest do\n  use ExUnit.Case\n\n  test \"adds\" do\n    assert 1 + 1 == 2\n  end\nend\n",
        );
    t.commit_all("mix");
    t
}

#[test]
fn elixir_ignore_markers_warn() {
    let t = mix_setup();
    t.write(
        "lib/a.ex",
        "# credo:disable-for-this-file\ndefmodule A do\n  @dialyzer {:nowarn_function, f: 0}\n  def f, do: :ok\nend\n",
    )
    .write(
        "test/math_test.exs",
        "defmodule MathTest do\n  use ExUnit.Case\n\n  @tag skip: \"flaky\"\n  test \"adds\" do\n    assert 1 + 1 == 3\n  end\nend\n",
    )
    .add(&["."]);
    let lines = pending(&t);
    for want in [
        "inline ignore: lib/a.ex adds 1x `credo:disable` (",
        "inline ignore: lib/a.ex adds 1x `@dialyzer` (",
        "inline ignore: test/math_test.exs adds 1x `@tag skip:` (",
    ] {
        assert!(
            lines.iter().any(|l| l.starts_with(want)),
            "{want}: {lines:?}"
        );
    }
}

#[test]
fn elixir_tool_configs_and_mix_aliases_are_guarded() {
    let t = mix_setup();
    t.write("test/test_helper.exs", "ExUnit.start(exclude: [:slow])\n")
        .add(&["test/test_helper.exs"]);
    assert_pending(
        &t,
        &["tool config edited: test/test_helper.exs (modified, "],
    );
    t.git(&["reset", "-q", "--hard"]);

    t.write(".formatter.exs", "[inputs: []]\n")
        .add(&[".formatter.exs"]);
    assert_pending(&t, &["tool config edited: .formatter.exs (modified, "]);
    t.git(&["reset", "-q", "--hard"]);

    t.write("mix.exs", &MIX_EXS.replace(", \"test\"]]", "]]"))
        .add(&["mix.exs"]);
    assert_pending(
        &t,
        &["mix.exs aliases or project settings changed: mix.exs (now "],
    );
    t.git(&["reset", "-q", "--hard"]);

    // A dependency or version bump is not a loosening.
    t.write(
        "mix.exs",
        &MIX_EXS
            .replace("~> 1.4", "~> 1.5")
            .replace("0.1.0", "0.2.0"),
    )
    .add(&["mix.exs"]);
    assert!(pending(&t).is_empty(), "{:?}", pending(&t));
}
