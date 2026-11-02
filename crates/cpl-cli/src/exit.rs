//! Exit codes (`spec/toolchain.md` 7.1, 7.4): every command ends with one
//! of three codes, which is exactly the envelope's `status` (T44).

use cpl_diag::json::Status;
use std::process::ExitCode;

/// The process exit code of a command.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Exit {
    /// `status: ok`: the command succeeded (warnings and infos alone).
    Ok = 0,
    /// `status: unsuccessful`: the command ran and reported errors or
    /// failures (check errors, a fault, a failed test, `--check` work).
    Unsuccessful = 1,
    /// `status: error`: a usage error, a missing project, unreadable input,
    /// a git failure, or an internal error.
    Error = 2,
}

impl Exit {
    /// The envelope status this exit code reports.
    pub fn status(self) -> Status {
        match self {
            Exit::Ok => Status::Ok,
            Exit::Unsuccessful => Status::Unsuccessful,
            Exit::Error => Status::Error,
        }
    }

    /// The numeric code (0, 1, 2).
    pub fn code(self) -> u8 {
        self as u8
    }
}

impl From<Status> for Exit {
    fn from(status: Status) -> Exit {
        match status {
            Status::Ok => Exit::Ok,
            Status::Unsuccessful => Exit::Unsuccessful,
            Status::Error => Exit::Error,
        }
    }
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> ExitCode {
        ExitCode::from(exit.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_exit_code_agree() {
        for status in [Status::Ok, Status::Unsuccessful, Status::Error] {
            let exit = Exit::from(status);
            assert_eq!(exit.status(), status);
            assert_eq!(exit.code(), status.exit_code());
        }
        assert_eq!(Exit::Ok.code(), 0);
        assert_eq!(Exit::Unsuccessful.code(), 1);
        assert_eq!(Exit::Error.code(), 2);
    }
}
