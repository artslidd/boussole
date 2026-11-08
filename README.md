# Boussole — jeu de devinette de villes françaises

A desktop game written in Rust with [Slint](https://slint.dev). The game secretly picks a
French city and shows it as a **?** in the middle of a compass dial. Five arrows leave the dial
in the true compass direction of five well-known cities. Type your guesses (with autocomplete):
each wrong guess tells you how far, and in which direction, the hidden city is. You get six
tries. The interface is in French.

![The start screen](docs/screenshots/01-start.png)

## Difficulty levels

The level is chosen **before each round** and is locked while the round runs (it is shown as a
badge in the header). The last choice is remembered for the session.

| Level | Arrow labels | Example |
|---|---|---|
| **Facile** — distances affichées | city name and distance, rounded to the km | `Lyon · 392 km` |
| **Difficile** — flèches seules | city name only | `Lyon` |

The guess history (distance and direction from your guess to the hidden city) is shown at both
levels.

| Facile | Difficile |
|---|---|
| ![A Facile round](docs/screenshots/03-facile-round.png) | ![A Difficile round](docs/screenshots/08-difficile-round.png) |

Same secret, same seed: only the labels differ. Arrow length never encodes the distance.

## Run

```sh
cargo run --release              # random games
cargo run --release -- --seed 42 # reproducible games: round k of the session uses seed 42 + k
```

Needs a recent stable Rust toolchain. Slint is built with its default winit backend and the
FemtoVG renderer; Qt is **not** required (the `fluent` std-widgets style is pinned in
`build.rs`). The first build compiles many crates and takes a few minutes.

## Multijoueur

Online rooms for 2–8 players (code + nickname; the host picks the mode — **Course**, a race on
the same city, or **Coopération**, turns on one shared round — and the settings). The server
is authoritative and never reveals the hidden city before the round ends.

Nobody has to rent a server: one player hosts from their own computer and friends join by
address and room code, over the LAN or through a free Cloudflare quick tunnel.

```sh
cargo run --release -p boussole-server                # LAN: ws://<your-ip>:7878/ws
cargo run --release -p boussole-server -- --tunnel    # + a public wss://….trycloudflare.com/ws (needs cloudflared)
```

- **[`docs/deploy.md`](docs/deploy.md)**: every way to run the server (from a player's
  computer, LAN, Docker/Podman, optional always-on host with HTTPS) and how to troubleshoot it.
- **[`docs/protocol.md`](docs/protocol.md)**: the wire protocol, rules, timers and scoring.

## How to play

1. Pick **Facile** or **Difficile**, then **Nouvelle partie**.
2. Read the arrows: each one points from the hidden city towards a labelled city. The small
   compass in the corner gives the orientation (N is up).
3. Type a city name. Suggestions appear to the right of the field (in the history's place, so
   the arrows stay visible); pick one.
4. After each guess, the history shows the distance and an arrow pointing from your guess
   towards the hidden city. Find it within six tries.
5. At the end the dial reveals the city. Choose a level and start again.

A name that is not in the list is refused, and a city you already tried is refused without
costing a try.

### Keyboard and mouse

Everything can be done with the keyboard alone, or with the mouse alone.

| Where | Keyboard | Mouse |
|---|---|---|
| Start / between rounds | `←` `→` choose the level, `Enter` starts, `Tab` reaches the button | click a level, click **Nouvelle partie** |
| Guess field | type, `↑` `↓` move in the suggestions, `Enter` submits the highlighted one (or the exact name), `Esc` closes the list, `Esc` again empties the field, `↓` on an empty field lists every city | click a suggestion; the chevron on the right of the field lists every city (scroll with the wheel) |

A held `Enter` never starts a round, and `Enter` is ignored for a moment after the result
appears, so the *Bravo !* / *Perdu !* screen cannot be skipped by accident.

## Jouer en ligne

Up to 8 players, in two modes chosen by the host in the lobby: **Course** (everybody races on
the same hidden city, with private feedback; points for fewer tries and speed; a live scoreboard
shows each player's tries but never a city name) and **Coopération** (one shared round: the
players take turns, six tries for the whole team, everyone sees every guess and who made it).
The hidden city never reaches a client before the round ends: the server decides, the client
only draws (see [`docs/protocol.md`](docs/protocol.md)).

From the start screen choose **Jouer en ligne**, type a **Pseudo** (1 to 16 characters), then:

| You want to… | Do this |
|---|---|
| **host** a game for friends | **Héberger une partie**. Your computer runs the server inside the app. Pick *Réseau local* (same Wi-Fi: friends use a `ws://192.168.…:7878/ws` address) or *Internet* (a free Cloudflare quick tunnel: friends use a `wss://….trycloudflare.com/ws` address; needs [`cloudflared`](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/), `brew install cloudflared` on macOS, no account). The lobby then shows the **room code** and the **Adresse à partager**, each with a **Copier** button. |
| **join** a friend's game | **Rejoindre**: paste the address you were given and type the 5-character room code (any case). Accepted addresses: `ws://…`, `wss://…`, a bare `host:port` (becomes `ws://host:port/ws`, port 7878 if omitted) or a pasted `https://….trycloudflare.com` (becomes `wss://….trycloudflare.com/ws`). |
| use a **standalone server** | Type its address and press **Créer une salle** (or **Rejoindre** with a code). |

![The En ligne screen](docs/screenshots/online/01-menu.png)

* **Lobby.** Only the host edits the settings (mode, level, rounds 1–10, seats 2–8, time per round
  or per turn) and presses **Lancer la partie** (needs two connected players); everybody else sees
  them greyed out. A host who leaves is asked first, because it closes the game for everybody and
  shuts the embedded server (and the tunnel) down.
* **Hosting trouble.** A fresh tunnel address is printed before it answers, so for the first seconds
  the lobby says *Partagez l’adresse dans quelques secondes*. If the tunnel dies later the header
  says *Tunnel perdu* and the Internet address is greyed out (players already on the local network
  keep playing); if the embedded server itself stops, the host is sent back to the menu.
* **Connection loss.** The header shows *Reconnexion…* with the seconds left; the app retries for
  up to 60 s with the session's resume token, and the server restores your place, your guesses and
  the clock. After that you are offered *Retour au menu*.
* **Errors** are in French and say what to do: *Serveur injoignable*, *Version incompatible —
  mettez le jeu à jour*, *Le port est déjà utilisé*, *Tunnel indisponible*, *Salle pleine* (a server
  over its connection cap), a full or unknown room, a nickname already taken… When the host
  closes the game (the server goes away: WebSocket close code 1001, or the host's port stops
  accepting connections right after a bare disconnect) everybody is sent back to the menu with
  *L’hôte a fermé la partie*: the client does not keep retrying for a game that cannot come back.
  A plain network drop, on the other hand, keeps retrying for 60 s. If `cloudflared` is missing the app explains how to install it and
  offers the local network instead.
* **Keyboard only** works from the start screen to the guesses: `Tab` moves between the nickname,
  the host/join forms and the buttons, `←` `→` change a choice, `Enter` confirms, and in a round
  the guess field has the focus (in Coopération it reopens by itself when it is your turn).
* The first time you host on the local network macOS may ask whether the app may accept incoming
  connections: allow it, or your friends cannot reach you.

| Lobby (host) | Course, Facile (live scoreboard) | Coopération, my turn |
|---|---|---|
| ![Lobby, host view](docs/screenshots/online/04-lobby-host-share.png) | ![Course round](docs/screenshots/online/08-course-facile.png) | ![Coopération, my turn](docs/screenshots/online/11-coop-my-turn.png) |

| End of a round | Final ranking | Reconnecting |
|---|---|---|
| ![Round result](docs/screenshots/online/13-round-result-course.png) | ![Final ranking](docs/screenshots/online/14-final-ranking.png) | ![Reconnecting](docs/screenshots/online/15-reconnecting.png) |

Everything fits the minimum window (800 × 600) with 8 players, see
[`docs/screenshots/online/`](docs/screenshots/online) (`17`–`19`). Running a server of your own
(Docker, always-on) is the server side's job: see [`docs/deploy.md`](docs/deploy.md).

**How it is built.** `src/net` is a background thread with a tokio runtime and a `tokio-tungstenite`
client (rustls with bundled roots, so `wss://` works) that says `hello`, keeps the socket alive
with `ping` and reconnects with the resume token; `src/online` is a pure reducer
(`apply(state, ServerMsg, now)`) plus the view-model each screen shows; `src/host.rs` is the
`Host` trait behind *Héberger* (starts, stops and health-checks the embedded server and its
tunnel) and `src/embedded_host.rs` its real implementation on top of `boussole-server`;
`src/online_ui.rs` is the glue with `ui/online*.slint`. The network thread never touches the UI:
events go through an inbox that the UI thread drains.

## Screenshots

All of them are produced by the real `.slint` UI rendered headlessly, with fixed secrets and
seeds (`cargo run --release --example screenshots`, see below).

| End of round (win) | End of round (loss) |
|---|---|
| ![Won](docs/screenshots/07-won.png) | ![Lost](docs/screenshots/09-lost.png) |

| Autocomplete (the board stays readable) | Tightest board: Porto-Vecchio at 800 × 600 |
|---|---|
| ![Autocomplete](docs/screenshots/04-autocomplete.png) | ![Porto-Vecchio, minimum window](docs/screenshots/12-porto-vecchio-800x600.png) |

More in [`docs/screenshots/`](docs/screenshots): start screen at the minimum size, the list of
every city, autocomplete at 800 × 600, the duplicate-guess notice, Facile at 800 × 600, the
*À propos* box.

## Project layout

A cargo workspace. The **root package `boussole` is the desktop client** (so `cargo run
--release` still launches the game); the crates below it are shared with the multiplayer
server being built (online rooms in two modes, *Course* and *Coopération*).

| Crate | What it is |
|---|---|
| `boussole` (root) | the Slint desktop client: the solo game and the online mode (host or join a room) |
| [`crates/boussole-core`](crates/boussole-core) | GUI-free, dependency-free kernel: `cities` (embedded dataset of 120 cities), `geo`, `search`, `game`; also `dataset_fingerprint()` / `DATASET_VERSION` |
| [`crates/boussole-protocol`](crates/boussole-protocol) | the wire protocol: message types, JSON codec, validation helpers, golden fixtures |
| [`crates/boussole-server`](crates/boussole-server) | the multiplayer server: the pure, IO-free **match engine** (`engine/`: rooms, Course and Coopération rules, timers, scoring, resume) behind an axum/WebSocket transport; usable as a binary or embedded as a library (`start`, `ServerHandle`, `tunnel`); `examples/bot` is a scripted player |

The protocol is specified in [`docs/protocol.md`](docs/protocol.md): transport, handshake,
every message with a JSON example, state diagrams, timers, scoring, error codes, versioning.
`boussole-server` never depends on Slint (`cargo tree -p boussole-server | grep -c slint` is 0).

- `src/` — the library `boussole` (the client). The core modules `cities`, `geo`, `search`
  and `game` live in `crates/boussole-core` and are re-exported here (`boussole::game::Round`
  works as before). It adds:
  - **presentation logic, still GUI-free and unit-tested:** `board` (arrow and label geometry,
    French texts), `session` (Setup → Playing → Ended state machine, autocomplete state),
    `text` (label measurement with the embedded font), `online` (the online reducer, its
    French texts and the view-model of each online screen), `net` (address normalisation and the
    WebSocket client), `host` (the `Host` trait behind *Héberger une partie*, with a fake for
    tests), `embedded_host` (the real one: the server embedded in the app, plus the tunnel), `clipboard`;
  - **Slint glue:** `app` (a `Session` behind the window), `online_ui` (the online mode's
    controller: state, connection, hosting, clipboard) and `headless` (a window without a screen,
    for screenshots and UI tests).
- `src/main.rs` — the game; `examples/` — `screenshots`, `real_window` (smoke test of the real
  winit window), `real_window_online` (the same for the online mode: hosts on loopback with the real
  server, a second client joins, a round starts, the host quits), `online_screenshots`,
  `sample_round` (prints a round to the terminal, `cargo run --example
  sample_round -- 42 --reveal`).
- `ui/*.slint` — the interface: `theme` (palette, fonts), `types`, `widgets` (buttons, level
  picker, dial, compass rose), `board`, `guess` (input, suggestions, history), `app`; the online
  screens are `online-state` (the `Online` global), `online-widgets`, `online-menu`,
  `online-lobby`, `online-round`, `online-podium` and `online` (header, banner, confirmation).
  `ui/fonts/` holds the two embedded fonts and their licences.
- `tests/` — `board_layout` (geometry over every secret and several window sizes), `ui_flow`
  (whole rounds played through the real UI); online: `online_reducer` and `online_view` (every
  `ServerMsg`, Course and Coopération sequences, what each screen shows), `net` (the client
  against a scripted WebSocket server in `tests/common`: handshake errors, reconnect with the
  token, giving up, the server going away), `online_flow` (the online screens driven by keys and
  clicks), `embedded_host` (the real embedded server: a two-client match, the port released,
  the tunnel path with an injected starter, never a real tunnel) and `online_hosting` (the whole
  "Héberger une partie" journey through the UI and the real server).

## Tests and quality gate

```sh
cargo test --workspace                  # every crate: unit tests, layout sweeps, UI and engine tests
cargo test -p boussole-server           # engine + network server + tunnel helper (real sockets, no GUI)
cargo test -p boussole-protocol         # codec + golden JSON fixtures (+ docs/protocol.md check)
cargo test -p boussole --lib board::    # one module of the client

# full gate, must be green before merging:
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo build --workspace --release

# after an intentional wire-format change, regenerate the golden fixtures, review the diff,
# and update docs/protocol.md (a test checks that it quotes every fixture verbatim):
BLESS=1 cargo test -p boussole-protocol --test golden

cargo run --release --example screenshots          # regenerate docs/screenshots/
cargo run --release --example online_screenshots   # regenerate docs/screenshots/online/
cargo run --release --example real_window -- /tmp/real  # opens the real window for ~2 s
cargo run --release --example real_window_online -- /tmp/real-online  # hosts + joins on loopback, ~8 s
```

The engine tests are deterministic (injected clock and randomness): they cover the handshake,
rooms, both game modes, timers, scoring and resume, check on 200 simulated matches that the
secret city never appears in a message before `round_ended` (and that Difficile never sends hint
distances), and throw more than 10 000 random messages at the engine.

The UI tests run without a display: `boussole::headless` installs a Slint platform backed by the
software renderer, injects key presses and clicks, and renders to memory. Mouse targets are
found through accessible labels (the tests use Slint's `i-slint-backend-testing` element
lookup, which is why `build.rs` enables debug info).

## Hint rules (see `crates/boussole-core/src/game.rs`)

Each round has exactly 5 hints from the `reference` cities, never the secret, 50–700 km away,
with bearings at least 40° apart (35° minimum, sorted clockwise). Secrets on the edge of France
(Ajaccio, Bastia, Porto-Vecchio, Brest, Quimper, Calais, Dunkerque, Strasbourg, Bayonne) have
all their neighbours in a narrow sector, so the spread and/or distance window is relaxed
deterministically (down to 15° and 1 000 km for Porto-Vecchio); the exact floors are documented
and tested in `game::tests::RELAXED` (in `boussole-core`). The board layout (`src/board.rs`) keeps every label
readable and inside the canvas down to a 15° gap: arrow *directions* are always exact, only a
label may slide aside along the circle to make room for its neighbour.

## Licences

- **Slint** is used under the **Royalty-Free Desktop, Mobile and Web Applications License**
  (Slint is also available under GPLv3 and a commercial licence). The licence asks for an
  attribution: the *À propos* box (button at the top right) shows Slint's `AboutSlint` badge.
- Fonts, embedded in the binary: **Young Serif** (titles and the dial) and **Barlow Semi
  Condensed** (everything else), both under the SIL Open Font License 1.1 — see
  `ui/fonts/OFL-*.txt`.
- "Boussole" is an original name and look; the game only borrows the *mechanic* of arrows
  pointing towards known cities.
