//! ass-ctl entry point: parse argv, connect to the IPC socket, print output.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Help is local and must work even outside a graphical session where
    // XDG_RUNTIME_DIR is unavailable.
    let command = match ass_ctl::command_name(&args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("ass-ctl: {error}");
            std::process::exit(2);
        }
    };
    if command.is_none() || matches!(command.as_deref(), Some("help" | "--help" | "-h")) {
        match ass_ctl::run(std::path::Path::new(""), &args) {
            Ok(output) => println!("{output}"),
            Err(e) => eprintln!("ass-ctl: {e}"),
        }
        return;
    }
    let socket = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => std::path::PathBuf::from(d).join("ass.sock"),
        None => {
            eprintln!("ass-ctl: $XDG_RUNTIME_DIR is unset; cannot locate ass.sock");
            std::process::exit(2);
        }
    };
    let result = match command.as_deref() {
        Some("subscribe") => ass_ctl::run_subscribe(&socket).map(|_| None),
        Some("subscribe-journal") => ass_ctl::run_subscribe_journal(&socket).map(|_| None),
        _ => ass_ctl::run(&socket, &args).map(Some),
    };
    match result {
        Ok(Some(output)) if !output.is_empty() => println!("{output}"),
        Ok(_) => {}
        Err(e) => {
            eprintln!("ass-ctl: {e}");
            std::process::exit(1);
        }
    }
}
