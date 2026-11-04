//! One module per command (`spec/toolchain.md` 1.10, 7.2): `check`, `fmt`,
//! `run`, `test`, `inspect`, `prove`, `diff`, `id` (`id assign`), and
//! `work` (`work check`, and the reserved `work run` and `work
//! integrate`). Every command follows the same skeleton: locate the
//! project (Section 7.1), load it, run the crate function, build the
//! [`Report`], print, exit. A command returns `Err(Failure)` when it stops
//! before it has a result of its own (a usage error, a load failure, check
//! errors before `run`, `test`, `prove`, or `work check`).

pub mod build;
mod check;
pub mod diff;
mod evolution;
pub mod fmt;
pub mod id;
pub mod inspect;
pub mod prove;
pub mod replay;
pub mod run;
pub mod test;
pub mod work;

use crate::cli::{Cli, Command, IdCommand, WorkCommand};
use crate::codes;
use crate::exit::Exit;
use crate::output::{Failure, Format, Output, Report};
use crate::project::Session;
use cpl_diag::json::DiagnosticJson;
use cpl_diag::{Diagnostic, Severity, SourceMap};
use serde::Serialize;

/// What every command gets: the file-system view and the printer.
pub struct Ctx {
    pub session: Session,
    pub out: Output,
}

impl Ctx {
    pub fn format(&self) -> Format {
        self.out.format
    }

    pub fn json(&self) -> bool {
        self.out.format != Format::Human
    }
}

/// Run one parsed command line and print its report.
pub fn dispatch(cli: Cli) -> Exit {
    let name = cli.command.name();
    let session = Session::new(cli.project.clone());
    let format = match (cli.json, cli.command.jsonl()) {
        (true, true) => {
            let out = Output::new(Format::Json);
            let d = codes::bad_argument("--json and --jsonl cannot be combined");
            return out.emit(session.refuse(d).into_report(name));
        }
        (true, false) => Format::Json,
        (false, true) => Format::Jsonl,
        (false, false) => Format::Human,
    };
    let ctx = Ctx {
        session,
        out: Output::new(format),
    };
    match &cli.command {
        Command::Check(args) => finish(&ctx, name, check::run(&ctx, args)),
        Command::Fmt(args) => finish(&ctx, name, fmt::run(&ctx, args)),
        Command::Run(args) => finish(&ctx, name, run::run(&ctx, args)),
        Command::Replay(args) => finish(&ctx, name, replay::run(&ctx, args)),
        Command::Build(args) => finish(&ctx, name, build::run(&ctx, args)),
        Command::Test(args) => finish(&ctx, name, test::run(&ctx, args)),
        Command::Id(id) => match &id.command {
            IdCommand::Assign(args) => finish(&ctx, name, id::run(&ctx, args)),
        },
        Command::Inspect(args) => finish(&ctx, name, inspect::run(&ctx, args)),
        Command::Query(args) => finish(&ctx, name, evolution::query(&ctx, args)),
        Command::Verify(args) => finish(&ctx, name, evolution::verify(&ctx, args)),
        Command::Edit(args) => finish(&ctx, name, evolution::edit(&ctx, args)),
        Command::Transaction(args) => finish(&ctx, name, evolution::transaction(&ctx, args)),
        Command::Conflicts(args) => finish(&ctx, name, evolution::merge(&ctx, args, None)),
        Command::Merge(args) => {
            finish(&ctx, name, evolution::merge(&ctx, &args.inputs, Some(args)))
        }
        Command::Explore(args) => finish(&ctx, name, evolution::explore(&ctx, args)),
        Command::Schema(args) => finish(&ctx, name, evolution::schema(&ctx, args)),
        Command::Migrate(args) => finish(&ctx, name, evolution::migrate(&ctx, args)),
        Command::Concurrent(args) => finish(&ctx, name, evolution::concurrent(&ctx, args)),
        Command::Prove(args) => finish(&ctx, name, prove::run(&ctx, args)),
        Command::Diff(args) => finish(&ctx, name, diff::run(&ctx, args)),
        Command::Work(w) => match &w.command {
            WorkCommand::Check(args) => finish(&ctx, name, work::check(&ctx, args)),
            WorkCommand::Run(_) | WorkCommand::Integrate(_) => {
                finish(&ctx, name, work::reserved(name))
            }
        },
    }
}

/// Print a command's report, or the report of its failure.
fn finish<T: Serialize>(ctx: &Ctx, name: &str, result: Result<Report<T>, Failure>) -> Exit {
    match result {
        Ok(report) => ctx.out.emit(report),
        Err(failure) => ctx.out.emit(failure.into_report(name)),
    }
}

/// Every diagnostic as its S49 object.
pub fn to_json(diagnostics: &[Diagnostic], sm: &SourceMap) -> Vec<DiagnosticJson> {
    diagnostics.iter().map(|d| d.to_json(sm)).collect()
}

/// The check diagnostics `cpl run` and `cpl test` report beside their own,
/// and `cpl prove` and `cpl work check` when the check fails: its errors
/// and warnings. The check's `hole` and `verification_result` infos are
/// `cpl check`'s reports (with the `dependencies` and receipts only `cpl
/// check` attaches) and are left out.
pub fn check_diagnostics_for_execution(diagnostics: &[Diagnostic]) -> Vec<Diagnostic> {
    diagnostics
        .iter()
        .filter(|d| d.severity != Severity::Info)
        .cloned()
        .collect()
}
