//! aegis-portal entry point: D-Bus-activated portal backend service.

use std::process::ExitCode;

fn main() -> ExitCode {
    env_logger::init();
    match aegis_portal::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::error!("aegis-portal: {error}");
            eprintln!("aegis-portal: {error}");
            ExitCode::FAILURE
        }
    }
}
