//! Boussole: the desktop game.
//!
//! `cargo run --release` opens the window. `--seed N` makes the games reproducible (round `k`
//! of the session uses seed `N + k`), which is handy for demos and manual checks.

use boussole::app::App;
use boussole::embedded_host::EmbeddedHost;
use std::sync::Arc;

fn main() -> Result<(), slint::PlatformError> {
    let app = match seed_from_args(std::env::args().skip(1)) {
        Ok(Some(first)) => App::with_seed_sequence(first)?,
        Ok(None) => App::with_time_seeds()?,
        Err(problem) => {
            eprintln!("boussole: {problem}; playing with random seeds instead");
            App::with_time_seeds()?
        }
    };
    // A hard exit (crash, kill -9) of an earlier run cannot stop the cloudflared it started: close
    // such leftovers now, in the background (only our own recorded tunnels, see `orphan_guard`).
    std::thread::spawn(|| {
        let closed = boussole::orphan_guard::reap_previous_runs();
        if closed > 0 {
            eprintln!("boussole: closed {closed} tunnel(s) left running by an earlier run");
        }
    });
    // "Héberger une partie" runs the Boussole server inside this process (and, on request, a
    // free Cloudflare tunnel). Nothing starts until the player asks for it.
    app.online().set_host(Arc::new(EmbeddedHost::new()));
    // ⌘Q goes through the same path as closing the window (the host is asked first, the server
    // and tunnel are stopped).
    if let Err(why) = app.online().install_quit_hook() {
        eprintln!("boussole: ⌘Q will not ask before closing a hosted game ({why})");
    }
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
