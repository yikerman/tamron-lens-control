#[cfg(not(target_os = "linux"))]
compile_error!("tlc supports Linux only");

mod cli;

use std::process::ExitCode;

fn main() -> ExitCode {
    match cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tlc: error: {error}");
            if matches!(error, tamron_lens_control::Error::NoDevice) {
                cli::print_driver_guidance();
            }
            ExitCode::FAILURE
        }
    }
}
