//! Boussole: the desktop game.
//!
//! `cargo run --release` opens the window. `--seed N` makes the games reproducible (round `k`
//! of the session uses seed `N + k`), which is handy for demos and manual checks.

use boussole::app::App;

fn main() -> Result<(), slint::PlatformError> {
    let seed = seed_from_args(std::env::args().skip(1));
    match seed {
        Some(first) => App::with_seed_sequence(first)?.run(),
        None => App::with_time_seeds()?.run(),
    }
}

/// The value after `--seed`, if any (`--seed 42` or `--seed=42`).
fn seed_from_args(args: impl IntoIterator<Item = String>) -> Option<u64> {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--seed" {
            return args.next().and_then(|value| value.parse().ok());
        }
        if let Some(value) = arg.strip_prefix("--seed=") {
            return value.parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::seed_from_args;

    fn parse(args: &[&str]) -> Option<u64> {
        seed_from_args(args.iter().map(|a| (*a).to_owned()))
    }

    #[test]
    fn seed_flag_is_parsed() {
        assert_eq!(parse(&["--seed", "42"]), Some(42));
        assert_eq!(parse(&["--seed=7"]), Some(7));
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&["--seed", "abc"]), None);
        assert_eq!(parse(&["--other", "1"]), None);
    }
}
