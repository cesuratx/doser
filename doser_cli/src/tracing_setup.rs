//! Tracing/logging initialization.

use crate::cli::FILE_GUARD;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// Build a file sink writer with optional rotation, storing the non-blocking guard in OnceLock.
fn file_layer(
    file: Option<&str>,
    rotation: Option<&str>,
) -> Option<tracing_appender::non_blocking::NonBlocking> {
    let path = file?;
    let p = std::path::Path::new(path);
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file_appender = match rotation.unwrap_or("never").to_ascii_lowercase().as_str() {
        "daily" => tracing_appender::rolling::daily(".", path),
        "hourly" => tracing_appender::rolling::hourly(".", path),
        _ => tracing_appender::rolling::never(".", path),
    };
    let (nb_writer, guard) = tracing_appender::non_blocking(file_appender);
    let _ = FILE_GUARD.set(guard);
    Some(nb_writer)
}

/// Initialize tracing once for the whole app.
pub fn init_tracing(json: bool, level: &str, file: Option<&str>, rotation: Option<&str>) {
    // Prefer RUST_LOG if set; otherwise use CLI level
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    let registry = tracing_subscriber::registry().with(filter);

    if json {
        let console = fmt::layer().json().with_target(false);
        if let Some(nb_writer) = file_layer(file, rotation) {
            let file_l = fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(nb_writer);
            registry.with(console).with(file_l).init();
        } else {
            registry.with(console).init();
        }
    } else {
        let console = fmt::layer().pretty().with_target(false);
        if let Some(nb_writer) = file_layer(file, rotation) {
            let file_l = fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(nb_writer);
            registry.with(console).with(file_l).init();
        } else {
            registry.with(console).init();
        }
    }
}
