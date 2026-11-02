//! The `cpl` command-line tool (`spec/toolchain.md` 1.10 and Section 7).
//!
//! | Module | Contents |
//! | - | - |
//! | `cli` | the clap definitions of Sections 7.1 and 7.2 |
//! | `commands` | one module per command: `check`, `fmt`, `run`, `test`, `inspect`, `prove`, `diff`, `id` (`id assign`), `work` (`work check`; `work run` and `work integrate`, reserved for Milestone 6) |
//! | `output` | the report every command builds, its human, `--json`, and `--jsonl` emission, the T46 order, the human rendering of 1.4.1 |
//! | `exit` | the exit codes of 7.4 and their envelope statuses |
//! | `project` | project discovery, loading, the repository and revisions, a commit's tree, the `LoadError` and `GitError` mappings |
//! | `json_arg` | the strict reader of `cpl run --arg JSON` |
//! | `codes` | the codes the command line raises |
//!
//! The CLI contains no logic that is not also reachable through the crate
//! APIs (T3): every command locates and loads the project, calls the crate
//! function, builds the envelope, prints, and exits. What the checker and
//! the runner leave to the command line (hole `dependencies`, receipts,
//! facet versions, target resolution, inspect responses, proofs) comes
//! from `cpl_index::Index`, semantic diffs from `cpl_index::semantic_diff`
//! over the facet maps of two revisions, and work validation from
//! `cpl_work::validate_work_file_with_policy`. The whole command runs
//! on the evaluation thread of `cpl_eval::on_eval_stack`, so that the
//! `serde_json` trees of deep values (a fault's `input`, a record's
//! payloads), which drop and serialize recursively, are dropped and
//! serialized on its large stack (semantics S38, toolchain 1.7).
//!
//! A command line clap refuses is `usage_bad_argument` (exit 2), printed
//! as an envelope when `--json` or `--jsonl` was written; `--help` and
//! `--version` print to stdout and exit 0. A panic is reported as
//! `internal_error` (exit 2) after the panic hook's trace on stderr.

mod cli;
mod codes;
mod commands;
mod exit;
mod json_arg;
mod output;
mod project;

use crate::exit::Exit;
use crate::output::{Failure, Format, Output};
use clap::error::ErrorKind;
use std::ffi::OsString;
use std::panic::AssertUnwindSafe;
use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let for_panic = argv.clone();
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || {
        cpl_eval::on_eval_stack(move || run(argv))
    }));
    match outcome {
        Ok(exit) => exit.into(),
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic without a message".to_string());
            let (command, json, jsonl) = cli::guess(&for_panic);
            let d = codes::internal("panic", format!("cpl panicked: {detail}"));
            output_for(json, jsonl)
                .emit(Failure::error(d).into_report(command))
                .into()
        }
    }
}

/// Parse and dispatch one command line.
fn run(argv: Vec<OsString>) -> Exit {
    match cli::parse(&argv) {
        Ok(cli) => commands::dispatch(cli),
        Err(error) => refused(&argv, error),
    }
}

fn output_for(json: bool, jsonl: bool) -> Output {
    Output::new(if json {
        Format::Json
    } else if jsonl {
        Format::Jsonl
    } else {
        Format::Human
    })
}

/// A command line clap refused: `usage_bad_argument`, exit 2, with clap's
/// message, its tip when it has one, and the command's usage line
/// ([`cli::usage`], not the usage in clap's error, which lists the flags
/// the refused line used) as notes; `--help` and `--version` are not
/// refusals.
fn refused(argv: &[OsString], error: clap::Error) -> Exit {
    if matches!(
        error.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
    ) {
        let _ = error.print();
        return Exit::Ok;
    }
    let (command, json, jsonl) = cli::guess(argv);
    let out = output_for(json, jsonl);
    if error.kind() == ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand {
        if out.format == Format::Human {
            let _ = error.print();
            return Exit::Error;
        }
        let d = codes::bad_argument("a command is required; run `cpl --help` for the commands");
        return out.emit(Failure::error(d).into_report(command));
    }
    let text = error.render().to_string();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or("invalid command line");
    let mut d = codes::bad_argument(first.strip_prefix("error: ").unwrap_or(first));
    for line in lines {
        if line.starts_with("tip:") {
            d = d.with_note(line, None);
        }
    }
    d = d.with_note(cli::usage(&command), None);
    out.emit(Failure::error(d).into_report(command))
}
