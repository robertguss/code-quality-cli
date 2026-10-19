//! The Usage spec (https://usage.jdx.dev): the whole interface as data, for
//! agents, shell completions and generated docs.

use clap::CommandFactory;

use super::Cli;

pub fn render() -> String {
    let mut cmd = Cli::command();
    let mut out = Vec::new();
    clap_usage::generate(&mut cmd, "qgate", &mut out);
    String::from_utf8(out).expect("usage spec is utf-8")
}
