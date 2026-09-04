mod preview;
mod profile;
mod render;
mod style;

fn main() {
    tessera_logging::init("info");
    if let Err(error) = preview::run() {
        log::error!("lock preview: {error}");
        eprintln!("tessera-lock-preview: {error}");
        std::process::exit(1);
    }
}
