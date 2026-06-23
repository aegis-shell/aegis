//! ass-ctl entry point: parse argv, connect to the IPC socket, print output.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let socket = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => std::path::PathBuf::from(d).join("ass.sock"),
        None => {
            eprintln!("ass-ctl: $XDG_RUNTIME_DIR is unset; cannot locate ass.sock");
            std::process::exit(2);
        }
    };
    match if args.first().map(String::as_str) == Some("subscribe") {
        ass_ctl::run_subscribe(&socket)
    } else {
        ass_ctl::run(&socket, &args).map(|_| ())
    } {
        Ok(()) => {}
        Err(e) => {
            eprintln!("ass-ctl: {e}");
            std::process::exit(1);
        }
    }
}
