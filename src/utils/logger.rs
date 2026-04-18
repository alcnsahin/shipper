use tracing_subscriber::{fmt, EnvFilter};

/// Initialise the global `tracing` subscriber.
///
/// Precedence (highest first):
///   1. `RUST_LOG` env var — raw `EnvFilter` directives, unchanged.
///   2. `--verbose` CLI flag → `"debug"`.
///   3. `config_level` from `~/.shipper/config.toml` (serde default `"info"`).
pub fn init(verbose: bool, config_level: &str) {
    let level = if verbose { "debug" } else { config_level };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();
}
