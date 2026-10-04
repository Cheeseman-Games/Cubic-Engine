//! `cubic-cli`, the binary.
//!
//! Everything worth testing lives in the library next door; this is the shell
//! around it — read the arguments, run the command, print the reason a run could
//! not happen, exit with a code a script can branch on.

use std::process::ExitCode;

fn main() -> ExitCode {
    match cubic_cli::run(std::env::args_os().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cubic-cli: {error}");
            ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1))
        }
    }
}
