//! One tracing setup for every binary.
//!
//! Filter with `VICTORY_LOG` (falls back to `RUST_LOG`), e.g.
//! `VICTORY_LOG=victory_loader=trace,info`. Debug and trace output exist
//! only in builds with the `diagnostics` feature; a plain release build
//! logs info and above whatever the filter asks for.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The most verbose level this build can log.
const CAP: LevelFilter = if cfg!(feature = "diagnostics") {
    LevelFilter::TRACE
} else {
    LevelFilter::INFO
};

pub fn init(default_level: &str) {
    let filter = std::env::var("VICTORY_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .ok()
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new(default_level));
    // `K6_LOG_FILE=<path>`: the log goes to that file, not the console. A
    // flood of warnings to a Windows console costs real time on the threads
    // that print; a file written line by line costs next to nothing.
    let to_file = std::env::var_os("K6_LOG_FILE")
        .filter(|p| !p.is_empty())
        .and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());
    let (writer, ansi) = match to_file {
        Some(file) => (
            tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::sync::Mutex::new(std::io::LineWriter::new(file))),
            false,
        ),
        None => (tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::io::stderr), true),
    };
    let _ = tracing_subscriber::registry()
        .with(CAP)
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(true).with_ansi(ansi).with_writer(writer))
        .try_init();
}
