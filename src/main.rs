//! Boussole: the desktop game.
//!
//! `cargo run --release` opens the window. `--seed N` makes the games reproducible (round `k`
//! of the session uses seed `N + k`), which is handy for demos and manual checks.

use boussole::app::App;

fn main() -> Result<(), slint::PlatformError> {
    let app = match seed_from_args(std::env::args().skip(1)) {
        Ok(Some(first)) => App::with_seed_sequence(first)?,
        Ok(None) => App::with_time_seeds()?,
        Err(problem) => {
            eprintln!("boussole: {problem}; playing with random seeds instead");
            App::with_time_seeds()?
        }
    };
    app.run()
}

/// The value after `--seed` (`--seed 42` or `--seed=42`), if any; an error message when the
/// flag has no usable value.
fn seed_from_args(args: impl IntoIterator<Item = String>) -> Result<Option<u64>, String> {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let value = if arg == "--seed" {
            args.next()
                .ok_or_else(|| "--seed needs a number, for example --seed 42".to_owned())?
        } else if let Some(value) = arg.strip_prefix("--seed=") {
            value.to_owned()
        } else {
            continue;
        };
        return value
            .parse()
            .map(Some)
            .map_err(|_| format!("invalid --seed value {value:?}, expected a whole number"));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::seed_from_args;

    fn parse(args: &[&str]) -> Result<Option<u64>, String> {
        seed_from_args(args.iter().map(|a| (*a).to_owned()))
    }

    #[test]
    fn seed_flag_is_parsed() {
        assert_eq!(parse(&["--seed", "42"]), Ok(Some(42)));
        assert_eq!(parse(&["--seed=7"]), Ok(Some(7)));
        assert_eq!(parse(&[]), Ok(None));
        assert_eq!(parse(&["--other", "1"]), Ok(None));
    }

    #[test]
    fn a_bad_seed_is_reported_not_ignored() {
        let message = parse(&["--seed", "abc"]).unwrap_err();
        assert!(message.contains("\"abc\""), "{message}");
        assert!(parse(&["--seed=-3"]).is_err());
        assert!(parse(&["--seed"]).unwrap_err().contains("needs a number"));
    }
}
