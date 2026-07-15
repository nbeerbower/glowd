mod protocol;
mod schedule;
mod server;
mod store;

const DEFAULT_PORT: u16 = 5578; // one above the LED control port, easy to remember

fn main() {
    let mut port = DEFAULT_PORT;
    let mut state_dir = store::default_state_dir();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" | "-p" => {
                port = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| {
                    eprintln!("--port needs a number (1-65535)");
                    std::process::exit(2);
                });
            }
            "--state-dir" => {
                state_dir = args.next().map(Into::into).unwrap_or_else(|| {
                    eprintln!("--state-dir needs a path");
                    std::process::exit(2);
                });
            }
            "--help" | "-h" => {
                println!("glowd — self-hosted web controller for MagicHome LED strips");
                println!();
                println!("Usage: glowd [--port PORT] [--state-dir DIR]");
                println!("  --port       listen port (default {DEFAULT_PORT})");
                println!("  --state-dir  where saved colors live (default $GLOWD_STATE_DIR or ~/.local/state/glowd)");
                return;
            }
            other => {
                eprintln!("unknown argument: {other} (try --help)");
                std::process::exit(2);
            }
        }
    }
    server::run(port, state_dir);
}
