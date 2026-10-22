//! qgate: a local, deterministic quality gate run from git hooks.

pub mod agent_hook;
pub mod baseline;
pub mod checks;
pub mod classify;
pub mod cli;
pub mod config;
pub mod git;
pub mod grammars;
pub mod guard;
pub mod hash;
pub mod hooks;
pub mod skill;
pub mod tools;
pub mod update;
