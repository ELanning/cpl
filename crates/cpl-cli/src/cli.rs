//! The command line of `spec/toolchain.md` 7.1 and 7.2, as clap derive
//! definitions: one struct per command, which doubles as its documented
//! contract (Section 1.3).
//!
//! ```text
//! cpl [--project DIR] [--json] <command> [args]
//! ```
//!
//! `--project` and `--json` are global: they may appear before or after
//! the command. Clap's own refusals (an unknown flag, a missing argument,
//! a value that does not parse as the flag's type) are `usage_bad_argument`
//! with exit code 2 ([`crate::main`]).

use clap::{ArgMatches, Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::path::PathBuf;

/// `cpl [--project DIR] [--json] <command> [args]`.
#[derive(Parser, Debug)]
#[command(
    name = "cpl",
    version,
    about = "The CPL toolchain (edition 0.1)",
    propagate_version = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
    /// The project root, a directory containing cpl.toml (default: the
    /// nearest ancestor of the current directory that contains one)
    #[arg(long, global = true, value_name = "DIR")]
    pub project: Option<PathBuf>,
    /// Emit exactly one JSON object (an envelope) on stdout and nothing else
    #[arg(long, global = true)]
    pub json: bool,
}

/// The commands of Section 7.1.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// typecheck the project
    Check(CheckArgs),
    /// format .cpl, .work, and .patch files
    Fmt(FmtArgs),
    /// run one function on constant arguments
    Run(RunArgs),
    /// dispatch or verify recorded events through a pure state machine
    Replay(ReplayArgs),
    /// run tests and properties
    Test(TestArgs),
    /// query the semantic index
    Inspect(InspectArgs),
    /// report verification status for one symbol
    Prove(ProveArgs),
    /// semantic diff between two revisions
    Diff(DiffArgs),
    /// id assign: insert missing @id annotations
    Id(IdArgs),
    /// work check: parse and validate a .work file against the project;
    /// work run / work integrate: reserved for Milestone 6
    Work(WorkArgs),
    /// build the project's C package (Milestone 2) or Python package (Milestone 3)
    Build(BuildArgs),
}

/// `cpl build [--target c|python] [--release] [--profile optimized|debug] [--sanitize LIST] [--artifact KIND]...
/// [--out DIR] [--lock] [--cc PATH] [--python PATH] [--jobs N] [--json]`
#[derive(Args, Debug, Default, Clone)]
pub struct BuildArgs {
    /// The target: c or python (Milestone 3); node is Milestone 4
    #[arg(long, value_name = "TARGET", default_value = "c")]
    pub target: String,
    /// Apply the release policy (holes, verification, the lock)
    #[arg(long)]
    pub release: bool,
    /// optimized (default) or debug
    #[arg(long, value_name = "PROFILE", default_value = "optimized", value_parser = ["optimized", "debug"])]
    pub profile: String,
    /// address,undefined (together or alone), or thread
    #[arg(long, value_name = "LIST")]
    pub sanitize: Option<String>,
    /// shared, static, or executable (repeatable; replaces [interop.c] artifacts)
    #[arg(long, value_name = "KIND", value_parser = ["shared", "static", "executable"])]
    pub artifact: Vec<String>,
    /// The output root (default `<project>/.cpl/build/<target>`)
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// Write cpl.lock with the target's entries after a successful build
    #[arg(long)]
    pub lock: bool,
    /// The C compiler, first in the compiler precedence
    #[arg(long, value_name = "PATH")]
    pub cc: Option<PathBuf>,
    /// The Python interpreter of --target python (default: CPL_PYTHON, else python3)
    #[arg(long, value_name = "PATH")]
    pub python: Option<PathBuf>,
    /// Parallel compilations (default: the number of CPUs)
    #[arg(long, value_name = "N")]
    pub jobs: Option<usize>,
}

impl Command {
    /// The envelope's `command` (Section 1.4.2): `"check"`, `"id assign"`,
    /// `"work check"`, ...
    pub fn name(&self) -> &'static str {
        match self {
            Command::Check(_) => "check",
            Command::Fmt(_) => "fmt",
            Command::Run(_) => "run",
            Command::Replay(_) => "replay",
            Command::Build(_) => "build",
            Command::Test(_) => "test",
            Command::Inspect(_) => "inspect",
            Command::Prove(_) => "prove",
            Command::Diff(_) => "diff",
            Command::Id(id) => match id.command {
                IdCommand::Assign(_) => "id assign",
            },
            Command::Work(work) => match work.command {
                WorkCommand::Check(_) => "work check",
                WorkCommand::Run(_) => "work run",
                WorkCommand::Integrate(_) => "work integrate",
            },
        }
    }

    /// True for the commands that accept `--jsonl` and set it (`check` and
    /// `test` only, Section 7.1).
    pub fn jsonl(&self) -> bool {
        match self {
            Command::Check(args) => args.jsonl,
            Command::Test(args) => args.jsonl,
            _ => false,
        }
    }
}

/// `cpl check [--json] [--jsonl] [--release] [--project DIR] [--lint NAME]... [--deny-warnings] [--lock]`
#[derive(Args, Debug, Default, Clone)]
pub struct CheckArgs {
    /// Stream one diagnostic object per line, then one line holding the
    /// envelope without `diagnostics`
    #[arg(long)]
    pub jsonl: bool,
    /// Release mode: fn-body holes and unsatisfied @prove declarations are
    /// errors, and a present cpl.lock must be fresh
    #[arg(long)]
    pub release: bool,
    /// Enable an off-by-default lint (id_missing)
    #[arg(long = "lint", value_name = "NAME")]
    pub lints: Vec<String>,
    /// Exit 1 when any warning is reported
    #[arg(long)]
    pub deny_warnings: bool,
    /// After a successful check, write cpl.lock for the current tree
    #[arg(long)]
    pub lock: bool,
}

/// `cpl fmt [--check] [--stdin KIND] [paths]`
#[derive(Args, Debug, Default, Clone)]
pub struct FmtArgs {
    /// Write nothing; exit 1 if any file would change, listing them one
    /// per line on stdout
    #[arg(long)]
    pub check: bool,
    /// Read one file of this kind from stdin and write the formatted text
    /// to stdout (no paths)
    #[arg(long, value_name = "KIND")]
    pub stdin: Option<StdinKind>,
    /// Files to format, or directories to format recursively (default:
    /// every .cpl, .work, and .patch file of the project's source set)
    #[arg(value_name = "PATH")]
    pub paths: Vec<PathBuf>,
}

/// The `KIND` of `cpl fmt --stdin`.
#[derive(ValueEnum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum StdinKind {
    Cpl,
    Work,
    Patch,
}

impl StdinKind {
    /// The formatter's file kind.
    pub fn file_kind(self) -> cpl_syntax::FileKind {
        match self {
            StdinKind::Cpl => cpl_syntax::FileKind::Cpl,
            StdinKind::Work => cpl_syntax::FileKind::Work,
            StdinKind::Patch => cpl_syntax::FileKind::Patch,
        }
    }
}

/// `cpl run <function> [arg]... [--arg JSON]... [--json] [--max-call-depth N]`
#[derive(Args, Debug, Default, Clone)]
pub struct RunArgs {
    /// The function: a qualified name (reader.pages.paginate_ints) or an @id
    #[arg(value_name = "FUNCTION")]
    pub function: String,
    /// An argument as a CPL constant expression ("[1, 2, 3]", 2,
    /// 'Page { id: PageId("p"), text: "t" }'); arguments bind to the
    /// parameters in order
    #[arg(value_name = "ARG", allow_negative_numbers = true)]
    pub args: Vec<String>,
    /// An argument as value JSON (may be mixed with constant-expression
    /// arguments; the order on the command line is the parameter order)
    #[arg(long = "arg", value_name = "JSON", allow_negative_numbers = true)]
    pub json_args: Vec<String>,
    /// The limit on active CPL function calls (default: run.max_call_depth of cpl.toml)
    #[arg(long, value_name = "N")]
    pub max_call_depth: Option<u32>,
    /// What evaluates the program: the reference evaluator (eval), the program's native driver (native), or both,
    /// compared (Milestone 2)
    #[arg(long, value_name = "ENGINE", default_value = "eval", value_parser = ["eval", "native", "both"])]
    pub engine: String,
    /// The interpreter of the Python workers (default: CPL_PYTHON, else python3; Milestone 3)
    #[arg(long, value_name = "PATH")]
    pub python: Option<PathBuf>,
    /// Every argument in command-line order (filled by [`parse`] from the
    /// argument indices clap records).
    #[arg(skip)]
    pub ordered: Vec<RunArgText>,
}

/// One `cpl run` argument as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunArgText {
    /// A bare argument: a constant expression.
    Const(String),
    /// `--arg JSON`.
    Json(String),
}

/// Commands are returned as data and never executed during replay.
#[derive(Args, Debug, Clone)]
pub struct ReplayArgs {
    #[arg(value_name = "MACHINE")]
    pub machine: String,
    /// Initial application state as value JSON
    #[arg(long, value_name = "JSON")]
    pub state: String,
    /// An ordered JSON array of events
    #[arg(long, value_name = "JSON", conflicts_with = "records")]
    pub events: Option<String>,
    /// Verify an ordered JSON array of transition receipts
    #[arg(long, value_name = "JSON")]
    pub records: Option<String>,
    #[arg(long, default_value = "eval", value_parser = ["eval", "native", "both"])]
    pub engine: String,
    #[arg(long, value_name = "N")]
    pub max_call_depth: Option<u32>,
    /// Time budget per machine call or boundary predicate, in milliseconds
    #[arg(long, value_name = "N", default_value = "5000", value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout_ms: u64,
}

/// `cpl test [selector] [--json] [--jsonl] [--seed N] [--cases N] [--name TEXT] [--id ID] [--fail-fast] [--max-shrinks N] [--timeout-ms N] [--receipt PATH]`
#[derive(Args, Debug, Default, Clone)]
pub struct TestArgs {
    /// A module path (tests.pagination) or namespace wildcard
    /// (tests.pagination.*): every test and property of that module and of
    /// every module below it (default: all)
    #[arg(value_name = "SELECTOR")]
    pub selector: Option<String>,
    /// Stream one diagnostic object per line as tests complete, then one
    /// line holding the envelope without `diagnostics`
    #[arg(long)]
    pub jsonl: bool,
    /// The run seed, a decimal u64 (default: 64 random bits from the OS,
    /// always reported)
    #[arg(long, value_name = "N")]
    pub seed: Option<u64>,
    /// Cases per property, at least 1 (default: test.cases of cpl.toml)
    // Reading of toolchain 7.2, which states no minimum (cpl-project's
    // `TestSection::cases` gives the reason): zero cases would report a
    // property `tested` without one evaluation, so `--cases 0` is refused
    // like any other value outside the flag's type (`usage_bad_argument`).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub cases: Option<u32>,
    /// Only the test or property with exactly this name text
    #[arg(long, value_name = "TEXT")]
    pub name: Option<String>,
    /// Only the test or property with this stable id
    #[arg(long, value_name = "ID")]
    pub id: Option<String>,
    /// Stop after the first failing test
    #[arg(long)]
    pub fail_fast: bool,
    /// Shrink budget per failure (default: test.max_shrinks of cpl.toml)
    #[arg(long, value_name = "N")]
    pub max_shrinks: Option<u32>,
    /// Wall-clock budget per body evaluation in milliseconds, 0 for none
    /// (default: test.timeout_ms of cpl.toml)
    #[arg(long, value_name = "N")]
    pub timeout_ms: Option<u64>,
    /// Also write the test receipt to this file
    #[arg(long, value_name = "PATH")]
    pub receipt: Option<PathBuf>,
    /// What evaluates the tests: the reference evaluator (eval), the program's native driver (native), or both,
    /// compared (Milestone 2)
    #[arg(long, value_name = "ENGINE", default_value = "eval", value_parser = ["eval", "native", "both"])]
    pub engine: String,
    /// The interpreter of the Python workers (default: CPL_PYTHON, else python3; Milestone 3)
    #[arg(long, value_name = "PATH")]
    pub python: Option<PathBuf>,
}

/// `cpl inspect <symbol> [--include LIST] [--json] [--since REV]`
#[derive(Args, Debug, Default, Clone)]
pub struct InspectArgs {
    /// A stable id, a qualified symbol path, or a bare name
    #[arg(value_name = "SYMBOL")]
    pub symbol: String,
    /// A comma list of signature, contract, effects, implementation, docs,
    /// callers, dependencies, obligations, tests, changes
    #[arg(long, value_name = "LIST")]
    pub include: Option<String>,
    /// The base revision of `changes` (default: HEAD~1)
    #[arg(long, value_name = "REV")]
    pub since: Option<String>,
}

/// `cpl prove <symbol> [--json]`
#[derive(Args, Debug, Default, Clone)]
pub struct ProveArgs {
    /// The function to report on
    #[arg(value_name = "SYMBOL")]
    pub symbol: String,
}

/// `cpl diff <range> [--semantic] [--exit-code] [--json]`
#[derive(Args, Debug, Default, Clone)]
pub struct DiffArgs {
    /// A...B (from the merge base), A..B, or A (against the working tree)
    #[arg(value_name = "RANGE")]
    pub range: String,
    /// The semantic diff (the only mode in Milestone 1)
    #[arg(long)]
    pub semantic: bool,
    /// Exit 1 when the diff is not empty
    #[arg(long)]
    pub exit_code: bool,
}

/// `cpl id <subcommand>`.
#[derive(Args, Debug, Clone)]
pub struct IdArgs {
    #[command(subcommand)]
    pub command: IdCommand,
}

/// The subcommands of `cpl id`.
#[derive(Subcommand, Debug, Clone)]
pub enum IdCommand {
    /// insert missing @id annotations
    Assign(IdAssignArgs),
}

/// `cpl id assign [paths] [--check] [--json]`
#[derive(Args, Debug, Default, Clone)]
pub struct IdAssignArgs {
    /// Report the declarations lacking an id without writing (exit 1 if any)
    #[arg(long)]
    pub check: bool,
    /// Files, or directories to cover recursively (default: every .cpl
    /// file of the project's source set)
    #[arg(value_name = "PATH")]
    pub paths: Vec<PathBuf>,
}

/// `cpl work <subcommand>`.
#[derive(Args, Debug, Clone)]
pub struct WorkArgs {
    #[command(subcommand)]
    pub command: WorkCommand,
}

/// The subcommands of `cpl work`.
#[derive(Subcommand, Debug, Clone)]
pub enum WorkCommand {
    /// parse and validate a .work file against the project
    Check(WorkFileArgs),
    /// reserved for Milestone 6
    Run(WorkFileArgs),
    /// reserved for Milestone 6
    Integrate(WorkIntegrateArgs),
}

/// `cpl work check <file>` and `cpl work run <file>`.
#[derive(Args, Debug, Default, Clone)]
pub struct WorkFileArgs {
    /// The .work file
    #[arg(value_name = "FILE")]
    pub file: PathBuf,
}

/// `cpl work integrate <change>`.
#[derive(Args, Debug, Default, Clone)]
pub struct WorkIntegrateArgs {
    /// The change to integrate
    #[arg(value_name = "CHANGE")]
    pub change: String,
}

/// Parse a command line (`argv[0]` included). The `cpl run` arguments are
/// put in command-line order ([`RunArgs::ordered`]): clap keeps the bare
/// arguments and the `--arg` values apart, and records where each was.
pub fn parse(argv: &[OsString]) -> Result<Cli, clap::Error> {
    let matches = Cli::command().try_get_matches_from(argv)?;
    let mut cli = Cli::from_arg_matches(&matches)?;
    if let (Command::Run(args), Some(("run", sub))) = (&mut cli.command, matches.subcommand()) {
        args.ordered = ordered_run_args(sub, args);
    }
    Ok(cli)
}

/// The bare arguments and `--arg` values of `cpl run` merged by position.
fn ordered_run_args(matches: &ArgMatches, args: &RunArgs) -> Vec<RunArgText> {
    let mut all: Vec<(usize, RunArgText)> = Vec::new();
    if let Some(indices) = matches.indices_of("args") {
        all.extend(
            indices
                .zip(&args.args)
                .map(|(i, v)| (i, RunArgText::Const(v.clone()))),
        );
    }
    if let Some(indices) = matches.indices_of("json_args") {
        all.extend(
            indices
                .zip(&args.json_args)
                .map(|(i, v)| (i, RunArgText::Json(v.clone()))),
        );
    }
    all.sort_by_key(|(i, _)| *i);
    all.into_iter().map(|(_, a)| a).collect()
}

/// What a command line that clap refused was probably asking for, for the
/// envelope of its usage error: the command name (`"cpl"` when none can
/// be told), whether `--json` was written, and whether `--jsonl` was
/// written for a command that has it (before `--`). `--jsonl` belongs to
/// `check` and `test` only (Section 7.1): written after any other command
/// it is itself the refused flag, and the refusal is printed as that
/// command prints, in the human form unless `--json` was written too.
pub fn guess(argv: &[OsString]) -> (String, bool, bool) {
    let args: Vec<String> = argv
        .iter()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .take_while(|a| a != "--")
        .collect();
    let json = args.iter().any(|a| a == "--json");
    let jsonl = args.iter().any(|a| a == "--jsonl");
    let mut words = Vec::new();
    let mut skip_value = false;
    for arg in &args {
        if skip_value {
            skip_value = false;
            continue;
        }
        if arg == "--project" {
            skip_value = true;
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        words.push(arg.as_str());
        if words.len() == 2 {
            break;
        }
    }
    let command = match words.as_slice() {
        ["id", "assign", ..] => "id assign".to_string(),
        ["work", sub @ ("check" | "run" | "integrate"), ..] => format!("work {sub}"),
        [first, ..]
            if [
                "check", "fmt", "run", "replay", "test", "inspect", "prove", "diff", "id", "work",
            ]
            .contains(first) =>
        {
            first.to_string()
        }
        _ => "cpl".to_string(),
    };
    let jsonl = jsonl && matches!(command.as_str(), "check" | "test");
    (command, json, jsonl)
}

/// The usage line of a command (`"check"`, `"id assign"`, `"cpl"` for the
/// top level) as clap renders it from the definitions alone:
/// `Usage: cpl run [OPTIONS] <FUNCTION> [ARG]...`. The note of a refused
/// command line uses it in place of the usage in clap's own error, which
/// lists the arguments the refused line happened to use (`Usage: cpl run
/// --json <FUNCTION> <ARG>...` for `cpl run --json f 1 --jsonl`) and so
/// restates the mistake instead of the command.
pub fn usage(command: &str) -> String {
    let mut root = Cli::command();
    root.build();
    let mut target = root.clone();
    for word in command.split(' ').filter(|w| *w != "cpl") {
        match target.find_subcommand(word) {
            Some(sub) => target = sub.clone(),
            None => break,
        }
    }
    target.render_usage().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<OsString> {
        std::iter::once("cpl")
            .chain(args.iter().copied())
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn the_definitions_are_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_flags_go_anywhere() {
        let cli = parse(&argv(&["check", "--json", "--project", "p"])).expect("parses");
        assert!(cli.json);
        assert_eq!(cli.project, Some(PathBuf::from("p")));
        let cli = parse(&argv(&["--json", "test", "--seed", "1"])).expect("parses");
        assert!(cli.json);
        match cli.command {
            Command::Test(t) => assert_eq!(t.seed, Some(1)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn run_arguments_keep_their_order() {
        let cli = parse(&argv(&[
            "run", "m.f", "1", "--arg", "[2]", "-3", "--arg", "{}", "\"x\"",
        ]))
        .expect("parses");
        let Command::Run(run) = cli.command else {
            panic!("run")
        };
        assert_eq!(run.function, "m.f");
        assert_eq!(
            run.ordered,
            [
                RunArgText::Const("1".into()),
                RunArgText::Json("[2]".into()),
                RunArgText::Const("-3".into()),
                RunArgText::Json("{}".into()),
                RunArgText::Const("\"x\"".into()),
            ]
        );
    }

    #[test]
    fn names_of_nested_commands() {
        let cli = parse(&argv(&["id", "assign", "--check"])).expect("parses");
        assert_eq!(cli.command.name(), "id assign");
        let cli = parse(&argv(&["work", "integrate", "c"])).expect("parses");
        assert_eq!(cli.command.name(), "work integrate");
    }

    #[test]
    fn refused_command_lines_are_guessed() {
        assert_eq!(
            guess(&argv(&["check", "--bogus", "--json"])),
            ("check".to_string(), true, false)
        );
        assert_eq!(
            guess(&argv(&["--project", "check", "id", "assign", "--nope"])),
            ("id assign".to_string(), false, false)
        );
        assert_eq!(
            guess(&argv(&["frobnicate"])),
            ("cpl".to_string(), false, false)
        );
        assert_eq!(
            guess(&argv(&["test", "--jsonl", "--", "--json"])),
            ("test".to_string(), false, true)
        );
        // `--jsonl` is check's and test's only (7.1).
        assert_eq!(
            guess(&argv(&["check", "--jsonl", "--bogus"])),
            ("check".to_string(), false, true)
        );
        for args in [
            &["run", "m.f", "--jsonl"][..],
            &["fmt", "--check", "--jsonl"],
            &["id", "assign", "--jsonl"],
            &["work", "check", "w.work", "--jsonl"],
            &["--jsonl"],
        ] {
            let (_, json, jsonl) = guess(&argv(args));
            assert!(!json && !jsonl, "{args:?}");
        }
        assert_eq!(
            guess(&argv(&["run", "m.f", "--jsonl", "--json"])),
            ("run".to_string(), true, false)
        );
    }

    #[test]
    fn usage_lines_come_from_the_definitions() {
        assert_eq!(usage("run"), "Usage: cpl run [OPTIONS] <FUNCTION> [ARG]...");
        assert_eq!(usage("check"), "Usage: cpl check [OPTIONS]");
        assert_eq!(
            usage("id assign"),
            "Usage: cpl id assign [OPTIONS] [PATH]..."
        );
        assert_eq!(
            usage("work integrate"),
            "Usage: cpl work integrate [OPTIONS] <CHANGE>"
        );
        assert_eq!(usage("cpl"), "Usage: cpl [OPTIONS] <COMMAND>");
        // No usage line names a flag the refused line happened to use.
        for command in [
            "check",
            "fmt",
            "run",
            "test",
            "inspect",
            "prove",
            "diff",
            "id assign",
            "work check",
            "work run",
            "work integrate",
        ] {
            let line = usage(command);
            assert!(
                line.starts_with(&format!("Usage: cpl {command} ")),
                "{line}"
            );
            assert!(!line.contains("--"), "{line}");
        }
    }

    #[test]
    fn bad_values_are_refused() {
        assert!(parse(&argv(&["test", "--seed", "-1"])).is_err());
        assert!(parse(&argv(&["test", "--seed", "1.5"])).is_err());
        assert!(parse(&argv(&["test", "--cases", "0"])).is_err());
        assert!(parse(&argv(&["test", "--cases", "1"])).is_ok());
        assert!(parse(&argv(&["test", "--cases", "4294967295"])).is_ok());
        assert!(parse(&argv(&["fmt", "--stdin", "toml"])).is_err());
        assert!(parse(&argv(&["check", "--frobnicate"])).is_err());
        assert!(parse(&argv(&["run"])).is_err());
    }
}
