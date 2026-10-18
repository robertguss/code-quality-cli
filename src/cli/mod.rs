//! Command definitions, help examples, error handling and exit codes.
//!
//! Exit codes: 0 pass, 1 findings, 2 usage or config error.

pub mod output;
pub mod usage_spec;

use std::fmt;
use std::path::PathBuf;

use clap::{Args, CommandFactory, Parser, Subcommand};

use crate::config::Stage;
pub use output::{Finding, Format, Report, Severity};

/// An error the caller can fix: bad flags, bad config, wrong version.
/// Always exits 2 and, when possible, says what to run instead.
#[derive(Debug)]
pub struct UsageError {
    pub message: String,
    pub hint: Option<String>,
}

impl UsageError {
    pub fn new(message: impl Into<String>) -> Self {
        UsageError {
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, "\n  {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for UsageError {}

const ROOT_EXAMPLES: &str = "Examples:
  qgate install
  qgate baseline create && qgate approve --dry-run
  qgate run --stage commit
  qgate run --stage push --format json";

const INSTALL_EXAMPLES: &str = "Examples:
  qgate install
  qgate install --dry-run
  qgate install --chain";

const RUN_EXAMPLES: &str = "Examples:
  qgate run --stage commit
  qgate run --stage push
  qgate run --stage commit --format json";

const BASELINE_EXAMPLES: &str = "Examples:
  qgate baseline create
  qgate baseline prune --dry-run
  qgate baseline prune";

const BASELINE_CREATE_EXAMPLES: &str = "Examples:
  qgate baseline create
  qgate baseline create --dry-run
  qgate baseline create --force";

const BASELINE_PRUNE_EXAMPLES: &str = "Examples:
  qgate baseline prune --dry-run
  qgate baseline prune";

const BASELINE_MERGE_EXAMPLES: &str = "Examples:
  qgate baseline merge base.txt ours.txt theirs.txt

`qgate install` registers this as a git merge driver (%O %A %B) for qgate.baseline.";

const APPROVE_EXAMPLES: &str = "Examples:
  qgate approve --dry-run
  qgate approve
  qgate approve --commit 3f2a9c1

Run this in your own terminal. Agents are blocked from running it.";

const AGENT_HOOK_EXAMPLES: &str = "Examples:
  echo '{\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"git status\"}}' | qgate agent-hook claude

`qgate install` registers this as a Claude Code PreToolUse hook.";

const USAGE_EXAMPLES: &str = "Examples:
  qgate usage
  qgate usage > qgate.usage.kdl && usage generate completion bash qgate -f qgate.usage.kdl";

/// Local quality gate for agent-written code. Runs as git hooks; never prompts.
#[derive(Debug, Parser)]
#[command(name = "qgate", version, after_help = ROOT_EXAMPLES)]
pub struct Cli {
    /// Output format.
    #[arg(long, global = true, value_enum, default_value_t = Format::Text)]
    pub format: Format,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Install the git hooks, the baseline merge driver and the Claude Code agent hook.
    #[command(after_help = INSTALL_EXAMPLES)]
    Install(InstallArgs),
    /// Run the checks for a commit or a push.
    #[command(after_help = RUN_EXAMPLES)]
    Run(RunArgs),
    /// Record, shrink or merge the baseline of existing violations.
    #[command(subcommand, after_help = BASELINE_EXAMPLES)]
    Baseline(BaselineCommand),
    /// Approve the exact pending loosening change (baseline growth, rule or tool changes).
    #[command(after_help = APPROVE_EXAMPLES)]
    Approve(ApproveArgs),
    /// Claude Code PreToolUse hook that blocks hook bypasses and gate tampering.
    #[command(subcommand, name = "agent-hook", after_help = AGENT_HOOK_EXAMPLES)]
    AgentHook(AgentHookCommand),
    /// Print the machine-readable Usage spec (usage.jdx.dev) for the whole CLI.
    #[command(after_help = USAGE_EXAMPLES)]
    Usage,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Keep existing hooks and run them after qgate instead of refusing.
    #[arg(long)]
    pub chain: bool,
    /// Show what would be written without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Which hook's checks to run.
    #[arg(long, value_enum)]
    pub stage: Stage,
    /// Remote whose default branch is the push base.
    #[arg(long, default_value = "origin")]
    pub remote: String,
    /// Read pushed refs from stdin in git pre-push format (set by the installed hook).
    #[arg(long)]
    pub hook_input: bool,
}

#[derive(Debug, Subcommand)]
pub enum BaselineCommand {
    /// Record every current violation in qgate.baseline. The result needs approval.
    #[command(after_help = BASELINE_CREATE_EXAMPLES)]
    Create {
        /// Replace an existing baseline.
        #[arg(long)]
        force: bool,
        /// Show the baseline without writing it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Lower or remove baseline entries that shrank or were fixed. Never raises or adds.
    #[command(after_help = BASELINE_PRUNE_EXAMPLES)]
    Prune {
        /// Show the change without writing it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Three-way merge of qgate.baseline, used as a git merge driver. Writes the result to OURS.
    #[command(after_help = BASELINE_MERGE_EXAMPLES)]
    Merge {
        base: PathBuf,
        ours: PathBuf,
        theirs: PathBuf,
    },
}

#[derive(Debug, Args)]
pub struct ApproveArgs {
    /// Approve the loosening introduced by this commit instead of the staged change.
    #[arg(long)]
    pub commit: Option<String>,
    /// Show what would be approved without recording it.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Subcommand)]
pub enum AgentHookCommand {
    /// Handle one Claude Code PreToolUse event from stdin.
    Claude,
}

/// What a command returns: a report to print, or raw output (merge driver, usage spec).
pub enum Outcome {
    Report(Report),
    Raw { stdout: String, code: i32 },
}

pub fn main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(e) => return clap_error(e, &args),
    };
    let format = cli.format;
    match dispatch(cli) {
        Ok(Outcome::Report(report)) => {
            print!("{}", report.render(format));
            report.exit_code()
        }
        Ok(Outcome::Raw { stdout, code }) => {
            print!("{stdout}");
            code
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            2
        }
    }
}

fn clap_error(e: clap::Error, args: &[String]) -> i32 {
    use clap::error::ErrorKind;
    match e.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            let _ = e.print();
            0
        }
        _ => {
            eprint!("{}", e.render());
            if let Some(examples) = examples_for(args) {
                eprintln!("\n{examples}");
            }
            2
        }
    }
}

/// The Examples block of the deepest subcommand named in `args`.
fn examples_for(args: &[String]) -> Option<String> {
    let mut cmd = Cli::command();
    let mut examples = cmd.get_after_help().map(|s| s.to_string());
    for arg in args.iter().skip(1) {
        let Some(sub) = cmd.find_subcommand(arg).cloned() else {
            continue;
        };
        if let Some(help) = sub.get_after_help() {
            examples = Some(help.to_string());
        }
        cmd = sub;
    }
    examples
}

fn dispatch(cli: Cli) -> anyhow::Result<Outcome> {
    match cli.command {
        Command::Usage => Ok(Outcome::Raw {
            stdout: usage_spec::render(),
            code: 0,
        }),
        Command::Baseline(BaselineCommand::Merge { base, ours, theirs }) => Ok(Outcome::Raw {
            stdout: String::new(),
            code: crate::baseline::merge_driver::run(&base, &ours, &theirs)?,
        }),
        Command::Baseline(BaselineCommand::Create { force, dry_run }) => Ok(Outcome::Report(
            crate::baseline::create(&repo()?, force, dry_run)?,
        )),
        Command::Install(args) => Ok(Outcome::Report(crate::hooks::install::install(
            &repo()?,
            args.chain,
            args.dry_run,
        )?)),
        Command::Run(args) => Ok(Outcome::Report(crate::hooks::run::run(
            &repo()?,
            args.stage,
            &args.remote,
            args.hook_input,
        )?)),
        Command::Approve(args) => Ok(Outcome::Report(crate::guard::approvals::approve(
            &repo()?,
            args.commit.as_deref(),
            args.dry_run,
        )?)),
        Command::Baseline(BaselineCommand::Prune { dry_run }) => {
            Ok(Outcome::Report(crate::baseline::prune(&repo()?, dry_run)?))
        }
        Command::AgentHook(AgentHookCommand::Claude) => Ok(Outcome::Raw {
            stdout: crate::agent_hook::run_claude()?,
            code: 0,
        }),
    }
}

fn repo() -> anyhow::Result<crate::git::Repo> {
    crate::git::Repo::discover(&std::env::current_dir()?)
}
