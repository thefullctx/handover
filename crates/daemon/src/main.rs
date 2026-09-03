//! Standalone Handover daemon.
//!
//! This binary is the headless/CLI mode: it runs the local HTTP API without
//! the GUI. The desktop app embeds the same crate in-process; normally you
//! run one or the other. Global hotkeys and the palette require the desktop
//! app — the daemon alone supports `handover status/send/...` from a
//! terminal.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use handover_daemon::{api, shared, Daemon};

fn main() {
    env_logger::init();

    let mut port = handover_daemon::DEFAULT_PORT;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                port = args.next().and_then(|p| p.parse().ok()).unwrap_or_else(|| {
                    eprintln!("usage: handover-daemon [--port PORT]");
                    std::process::exit(2);
                });
            }
            "--help" | "-h" => {
                println!("handover-daemon — Handover background daemon (headless mode)");
                println!("usage: handover-daemon [--port PORT]");
                println!("Serves the local HTTP API on 127.0.0.1 (default port 47444).");
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let daemon = match Daemon::load() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("handover-daemon: {e}");
            std::process::exit(1);
        }
    };

    let shared = shared(daemon);
    let shutdown = Arc::new(AtomicBool::new(false));

    match api::serve(shared.clone(), port, Arc::clone(&shutdown)) {
        Ok(handle) => println!(
            "Handover daemon listening on http://127.0.0.1:{} (pid {})",
            handle.port,
            std::process::id()
        ),
        Err(e) => {
            eprintln!("handover-daemon: {e}");
            std::process::exit(1);
        }
    }

    // Run until SIGINT/SIGTERM or a /quit request. Ctrl-C will also end us,
    // which is fine for headless mode.
    while !shutdown.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    println!("Handover daemon stopped.");
}
