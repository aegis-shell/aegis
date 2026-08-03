mod identity;
mod preview;
mod render;

fn main() {
    aegis_logging::init("info");
    if let Err(error) = preview::run() {
        log::error!("lock preview: {error}");
        eprintln!("aegis-lock-preview: {error}");
        std::process::exit(1);
    }
}
