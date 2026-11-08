# Boussole — jeu de devinette de villes françaises

Rust game: the hidden French city is drawn as a "?" in the centre, with arrows (true compass
bearing) to 5 well-known cities. Guess it in 6 tries; each wrong guess shows the distance and
direction from your guess to the answer.

Difficulty: **Facile** (arrow labels also show km, `Lyon · 392 km`) or **Difficile** (names only).

## Layout

- `src/lib.rs` — UI-independent core (no GUI types, no dependencies):
  `cities` (embedded dataset), `geo` (distance, bearing, compass), `search` (name
  normalization + autocomplete), `game` (difficulty, hints, rounds, guesses).
- `src/main.rs` — placeholder binary that prints a sample round (the GUI comes in ticket 02).

## Run

```sh
cargo run                    # sample round with a time-based seed (secret hidden)
cargo run -- 42 --reveal     # fixed seed 42, also print the secret
```

## Tests and quality gate

```sh
cargo test                   # all unit tests (dataset, geo, search, rounds, guesses)
cargo test game::            # only one module (cities::, geo::, search::, game::)

# full gate, must be green before merging:
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && cargo build --release
```

## Hint rules (see `src/game.rs`)

Each round has exactly 5 hints from the `reference` cities, never the secret, 50–700 km away,
with bearings at least 40° apart (35° minimum, sorted clockwise). Secrets on the edge of France
(Ajaccio, Bastia, Porto-Vecchio, Brest, Quimper, Calais, Dunkerque, Strasbourg, Bayonne) have
all their neighbours in a narrow sector, so the spread and/or distance window is relaxed
deterministically; the exact floors are documented and tested in `game::tests::RELAXED`.
