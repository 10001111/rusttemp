//! The direct `cpu` and `gpu` commands.
//!
//! `src/bin/cpu.rs` and `src/bin/gpu.rs` call [`cpu::main`] / `gpu::main`. All
//! hardware access goes through the shared library modules.

pub mod cpu;
pub mod gpu;

use crate::errors::Result;

/// Smallest accepted `--interval` in seconds.
pub const MIN_INTERVAL_S: f64 = 0.25;
/// Largest accepted `--interval` in seconds (1 hour).
pub const MAX_INTERVAL_S: f64 = 3600.0;

/// Parses a `--interval` value in seconds (decimals allowed).
pub fn parse_interval(s: &str) -> std::result::Result<f64, String> {
    let v: f64 = s
        .trim()
        .parse()
        .map_err(|_| format!("`{s}` is not a number of seconds"))?;
    if v.is_finite() && (MIN_INTERVAL_S..=MAX_INTERVAL_S).contains(&v) {
        Ok(v)
    } else {
        Err(format!(
            "must be between {MIN_INTERVAL_S} and {MAX_INTERVAL_S} seconds"
        ))
    }
}

/// Exits the process with the right code for `result`.
pub fn finish(program: &str, result: Result<()>) -> ! {
    match result {
        Ok(()) => std::process::exit(0),
        // The reader closed the pipe (e.g. `cpu --json | head -1`): not an error.
        Err(e) if e.is_broken_pipe() => std::process::exit(0),
        Err(e) => {
            eprintln!("{program}: error: {e}");
            std::process::exit(e.exit_code());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_parsing() {
        assert_eq!(parse_interval("2"), Ok(2.0));
        assert_eq!(parse_interval("0.5"), Ok(0.5));
        assert_eq!(parse_interval("0.25"), Ok(0.25));
        assert!(parse_interval("0.1").is_err());
        assert!(parse_interval("3601").is_err());
        assert!(parse_interval("abc").is_err());
        assert!(parse_interval("NaN").is_err());
        assert!(parse_interval("-1").is_err());
    }
}
