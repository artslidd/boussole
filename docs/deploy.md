# Running a Boussole server

The multiplayer server is one small binary, `boussole-server` (axum + WebSocket). It keeps
everything in memory: no database, no accounts, nothing to back up. There are four ways to
run it, from "no setup at all" to "always on":

| | Who | Needs | Players connect to |
|---|---|---|---|
| [0. From a player's computer](#0-free-play-from-a-players-computer-recommended) | one player hosts, friends anywhere | `cargo` + free `cloudflared` (a button in the game is planned) | `wss://<random-words>.trycloudflare.com/ws` |
| [1. LAN](#1-lan-play) | people on the same network | `cargo` (or the game, once hosting lands) | `ws://<host-ip>:7878/ws` |
| [2. One container](#2-docker--podman-single-container) | a machine that already runs containers | Docker or Podman | `ws://<host-ip>:7878/ws` |
| [3. Always-on server](#3-optional-always-on-remote-host) | a small VPS | Docker/Podman + a domain name | `wss://<your-domain>/ws` |

The wire protocol is specified in [`protocol.md`](protocol.md). Player-facing text in the game
is French; this guide is for whoever runs the server.

## 0. Free play from a player's computer (recommended)

One player runs the server on their own computer and shares an address; nobody pays for a
server and nobody needs an account.

### From the game (planned, ticket 05: not in the game yet)

The plan is a **Héberger une partie** button in the game's online menu that starts this same
server *inside the app* (`boussole_server::start`) and shows the addresses to share: the LAN
address and, if you ask for it, the public tunnel address below. Friends would then choose
**Rejoindre** and enter the address and the room code. Until that lands, use the command line
below; the server and the tunnel are the same.

### From the command line

```sh
cargo run --release -p boussole-server -- --tunnel
```

```text
Boussole server listening on 0.0.0.0:7878
  local:  ws://127.0.0.1:7878/ws
  LAN:    ws://192.168.1.26:7878/ws
  public: wss://beds-rugby-job-strips.trycloudflare.com/ws  (Cloudflare quick tunnel; the URL changes at every start)
```

Share the `public:` address (and, once you create a room in the game, its 5-letter code).
`Ctrl-C` stops the server **and** the tunnel. With a tunnel you do not need to accept incoming
connections on your machine, so you can listen on the loopback interface only, which also avoids
the macOS firewall prompt:

```sh
BOUSSOLE_BIND=127.0.0.1:7878 cargo run --release -p boussole-server -- --tunnel
```

### How the quick tunnel works

`--tunnel` runs [`cloudflared`](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/do-more-with-tunnels/trycloudflare/)
(`cloudflared tunnel --no-autoupdate --url http://127.0.0.1:<port>`), which opens an outgoing
connection to Cloudflare and gets back a random `https://<words>.trycloudflare.com` address that
forwards to your server, WebSocket included. Things to know:

- **No account, no cost, no configuration.**
- **The address is random and changes every time you start it.** Send the new one to your
  friends each time. It is printed as soon as `cloudflared` has it, which can be a few seconds
  before it works (in the test runs it answered at once); if friends get an error right away,
  ask them to retry in a moment.
- **Testing grade:** Cloudflare gives quick tunnels no uptime guarantee (and limits concurrent
  requests). It is meant for game nights, not for a permanent service. For that, use
  [section 3](#3-optional-always-on-remote-host).
- **Anyone who has the address can reach your server.** What keeps a game yours is the room:
  a random 5-letter code that you share only with friends (guessing is throttled: a connection
  that asks for unknown codes too often is dropped), at most 8 players per room, and limits on
  frame size, message rate and silent connections (see the [reference](#limits)). Those limits
  slow an abuser down; they do not make the address secret. Someone who has it can still open
  a few connections or rooms (a server hosted from the game allows 16 connections and 4 rooms,
  4 connections per visitor address) and make your friends' attempts fail for a while: idle
  connections that never enter a room are dropped after 60 s and rooms expire after 30 minutes
  of silence, but a determined abuser can keep refilling them. **If the address leaks, stop the
  tunnel and start a new one**: the address is random and changes every time. Do not keep the
  tunnel open longer than you play. People living behind one internet connection (one flat,
  one office) count as one visitor address: more than 4 of them cannot join the same
  tunnelled server, they can use the LAN address instead.
- The traffic between your friends and Cloudflare is encrypted (`wss://`); Cloudflare can see it.
  There is nothing secret in a game of Boussole, but you should know.
- `--tunnel` passes `cloudflared` an empty configuration file: a `cloudflared` set up for
  something else on the same machine (a named tunnel, `proxy-dns`, ...) would otherwise make
  quick tunnels refuse to start (*"running a quick tunnel with proxy-dns is not supported"*).
  The program that is already running keeps running; it is never touched.
- **If the server or the game is killed hard** (`kill -9`, a crash, a power cut) it cannot stop
  the `cloudflared` it started, and that process keeps the old address alive. `Ctrl-C` and
  `SIGTERM` stop both. To clean up a stray one, list yours by the port it forwards to, then
  stop that process only:

  ```sh
  pgrep -fl 'cloudflared.*--url http://127.0.0.1:7878'   # use your port
  kill <pid>
  ```

  **Never `pkill cloudflared` (or `pkill -f cloudflared`)**: other `cloudflared` processes,
  such as a system service, may be running on the same machine for other purposes.
  The server also leaves a tiny `boussole-cloudflared-*.yml` file in the temporary directory
  after such a crash; it is empty and harmless.
- If starting fails, the server stops with the reason (`cloudflared` missing, no network, no URL
  after 20 seconds); it never silently continues without the tunnel you asked for.

### Installing `cloudflared`

| System | Command |
|---|---|
| macOS | `brew install cloudflared` |
| Debian / Ubuntu | `.deb` from the [releases page](https://github.com/cloudflare/cloudflared/releases/latest) (`sudo dpkg -i cloudflared-linux-amd64.deb`) |
| Windows | `winget install --id Cloudflare.cloudflared`, or the `.msi` from the releases page |
| Others | the [downloads page](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/) |

`cloudflared --version` should print a version. If it is not on your `PATH` the server reports
that `cloudflared` is not installed.

### Alternative: Tailscale Funnel

If you already use [Tailscale](https://tailscale.com), **Funnel** publishes a local port on a
stable `https://<machine>.<tailnet>.ts.net` address (enable Funnel for your tailnet first, see
Tailscale's documentation):

```sh
cargo run --release -p boussole-server          # BOUSSOLE_BIND=127.0.0.1:7878 is enough
tailscale funnel 7878                           # in another terminal
```

Friends enter `wss://<machine>.<tailnet>.ts.net/ws`. The address is stable, which is its
advantage over a quick tunnel; it needs a Tailscale account.

## 1. LAN play

For people on the same Wi-Fi or Ethernet network:

```sh
cargo run --release -p boussole-server
```

The server listens on `0.0.0.0:7878` and prints the addresses of this machine:

```text
Boussole server listening on 0.0.0.0:7878
  local:  ws://127.0.0.1:7878/ws
  LAN:    ws://192.168.1.26:7878/ws
```

Everyone else on the network enters the `LAN:` address (`ws://<ip>:7878/ws`) in the game. To
find the address yourself:

| System | Command |
|---|---|
| macOS | `ipconfig getifaddr en0` (Wi-Fi; try `en1` on some machines), or `ifconfig \| grep "inet "` |
| Linux | `hostname -I`, or `ip -4 addr` |
| Windows | `ipconfig` and read the *IPv4 Address* of your adapter |

- **macOS firewall:** the first time a program listens on `0.0.0.0`, macOS may ask *"Do you
  want the application to accept incoming network connections?"* — click **Allow**, or nobody
  else will be able to connect. Binding to `127.0.0.1` (the tunnel setup) never asks.
- **Other firewalls:** allow incoming TCP on port 7878 (`BOUSSOLE_BIND=0.0.0.0:<port>` changes it).
- Check from another machine: `curl http://<ip>:7878/healthz` prints `ok`.
- Guest networks and some company Wi-Fi isolate devices from each other; use a tunnel then.

To play without a graphical client, the repository has a scripted player. One bot creates the
room and starts the match when enough players are in; the others join with the printed code:

```sh
cargo run --release -p boussole-server --example bot -- --url ws://192.168.1.26:7878/ws --create --mode course --players 3 --nickname Ada
cargo run --release -p boussole-server --example bot -- --url ws://192.168.1.26:7878/ws --room K7QX2 --nickname Bob
```

`--mode coop` plays Coopération; `--help` lists the other flags.

## 2. Docker / Podman, single container

The image is a multi-stage build (Rust, then a distroless runtime). It is about 40 MB, runs
as a non-root user and contains only the server binary. From the repository root:

```sh
docker build -t boussole-server .
docker run -d --name boussole --restart unless-stopped -p 7878:7878 boussole-server
```

With Podman, build in Docker image format so that the image keeps its `HEALTHCHECK`
(otherwise Podman warns that it is ignored, harmlessly):

```sh
podman build --format docker -t boussole-server .
podman run -d --name boussole --restart unless-stopped -p 7878:7878 boussole-server
```

| Task | Command |
|---|---|
| Health | `curl http://localhost:7878/healthz` (prints `ok`); `docker inspect --format '{{.State.Health.Status}}' boussole` |
| Logs | `docker logs -f boussole` |
| Stop (graceful, clients are told goodbye) | `docker stop boussole` |
| Settings | `-e BOUSSOLE_MAX_ROOMS=100 -e RUST_LOG=debug …` (see [Settings](#settings)) |
| Harden further | `--read-only --cap-drop ALL --security-opt no-new-privileges` (the server needs none of them) |

The image contains no shell and no `curl`; its `HEALTHCHECK` is the server's own
`boussole-server --healthcheck`. The `--tunnel` flag is for running the binary on your own
computer: the image does not contain `cloudflared`.

### With Compose

[`deploy/compose.yaml`](../deploy/compose.yaml) wraps the same image with sane defaults
(restart policy, health check, read-only file system, no capabilities):

```sh
docker compose -f deploy/compose.yaml --profile lan up -d --build     # ws://<host>:7878/ws
docker compose -f deploy/compose.yaml --profile lan down
```

`podman compose` (with `podman-compose` or `docker-compose` installed) takes the same
arguments. `BOUSSOLE_PORT=9000` publishes another host port.

## 3. Optional: always-on remote host

A small VPS (the smallest plan is enough for a few dozen rooms; this has not been load-tested
yet), a domain name and Docker or Podman. [Caddy](https://caddyserver.com) sits in front of the
server, gets a free Let's Encrypt certificate by itself and forwards WebSockets, so players
connect to `wss://<your-domain>/ws`.

1. **DNS:** create an `A` record (and `AAAA` if you have IPv6) for the domain, for instance
   `boussole.example.org`, pointing at the server's public address. Wait until
   `dig +short boussole.example.org` returns it.
2. **Firewall:** allow incoming `80/tcp` (certificate challenge and redirect to HTTPS),
   `443/tcp` and `443/udp` (HTTP/3). **Do not open 7878**: in this profile the server is only
   reachable from Caddy, through the container network.
3. **Get the code and start:**

   ```sh
   git clone <your copy of the repository> boussole && cd boussole
   export BOUSSOLE_DOMAIN=boussole.example.org
   export ACME_EMAIL=you@example.org
   docker compose -f deploy/compose.yaml --profile tls up -d --build
   ```

   (Or put both variables in a `.env` file next to `deploy/compose.yaml`.) The first start
   obtains the certificate within a minute.
4. **Check:** `curl https://boussole.example.org/healthz` prints `ok`. In the game, join
   `wss://boussole.example.org/ws`.

Caddy only forwards `/ws` and `/healthz`; every other path answers 404. Its certificates live
in the `caddy_data` volume: keep it between restarts so that certificates are not requested
again (Let's Encrypt rate-limits that).

**Operating it**

| Task | Command |
|---|---|
| Update | `git pull && docker compose -f deploy/compose.yaml --profile tls build && docker compose -f deploy/compose.yaml --profile tls up -d` |
| Logs | `docker compose -f deploy/compose.yaml --profile tls logs -f server` (and `caddy`) |
| Health | `docker compose -f deploy/compose.yaml --profile tls ps` (shows *healthy*), `curl https://$BOUSSOLE_DOMAIN/healthz` |
| Stop | `docker compose -f deploy/compose.yaml --profile tls down` |
| JSON logs | `BOUSSOLE_LOG_FORMAT=json` in the environment before `up -d` |

- **Updating drops running games:** all state is in memory, so a restart ends every room and
  invalidates every resume token. Update when nobody is playing.
- **Backups: none needed.** The server is stateless; the only thing worth keeping is the
  `caddy_data` volume (certificates), and Caddy can issue new ones.
- Behind Caddy, the server's logs show `peer=` (Caddy's container address) and `client=` (the
  player's address as Caddy reports it, trusted because the compose file sets
  `BOUSSOLE_TRUST_PROXY_HEADERS` for this profile).
- `deploy/compose.yaml` bounds every container (`pids_limit`, `mem_limit`, rotated logs of at
  most 3 × 10 MB) and drops all Linux capabilities (Caddy keeps only `NET_BIND_SERVICE`).
- **Pinning image versions (optional).** `caddy:2`, `rust:1.99-slim-bookworm` and
  `gcr.io/distroless/cc-debian12:nonroot` are moving tags, convenient and patched but not
  reproducible. To pin them, look up the digest and append it:

  ```sh
  podman manifest inspect docker.io/library/caddy:2 | grep -m1 '"digest"'   # or: docker buildx imagetools inspect caddy:2
  # deploy/compose.yaml:  image: caddy:2@sha256:<digest>
  # Dockerfile:           FROM rust:1.99-slim-bookworm@sha256:<digest> AS build
  ```

  Pinned images stay as they are until you change the digest, so you also have to watch for
  security updates yourself.
- **Rootless Podman** cannot publish ports below 1024: run the TLS profile as root, lower
  `net.ipv4.ip_unprivileged_port_start`, or use Docker.
- To use another reverse proxy, forward `/ws` with HTTP/1.1 upgrade headers, for example
  (nginx, not tested here):

  ```nginx
  location /ws {
      proxy_pass http://127.0.0.1:7878;
      proxy_http_version 1.1;
      proxy_set_header Upgrade $http_upgrade;
      proxy_set_header Connection "upgrade";
      proxy_read_timeout 120s;
  }
  ```

  The server pings every connection every 20 s, so any proxy timeout above that is safe.

## Reference

### Settings

| Environment variable | Default | Meaning |
|---|---|---|
| `BOUSSOLE_BIND` | `0.0.0.0:7878` | listen address (`127.0.0.1:7878` for loopback only; port `0` picks a free one) |
| `BOUSSOLE_MAX_CONNECTIONS` | `2000` | simultaneous WebSocket connections (more are refused with HTTP 503) |
| `BOUSSOLE_MAX_ROOMS` | `500` | simultaneous rooms (`create_room` beyond it is answered `invalid_state`) |
| `BOUSSOLE_MAX_CONNECTIONS_PER_IP` | `20` | simultaneous connections from one client address (more are refused with HTTP 429); `4` when hosted from the game |
| `BOUSSOLE_TRUST_PROXY_HEADERS` | `false` | believe `Cf-Connecting-Ip` / `X-Forwarded-For` from any peer (default: only from loopback peers); see [Limits](#limits) |
| `RUST_LOG` | `info` | log filter, e.g. `debug`, `boussole_server=debug` |
| `BOUSSOLE_LOG_FORMAT` | text | `json` for one JSON object per log line |

Malformed values stop the server at startup with a message (exit code 2). Flags: `--tunnel`,
`--healthcheck` (exit 0 if `/healthz` answers 200 on the configured port; used by the
container), `--help`.

When the server is embedded in an app (the planned *Héberger une partie*), it uses small limits
instead (16 connections, 4 rooms, 4 connections per client address), chosen by
`ServerConfig::for_host_mode`.

### Endpoints

| Path | Answer |
|---|---|
| `GET /ws` | WebSocket upgrade; the game protocol ([`protocol.md`](protocol.md)) |
| `GET /healthz` | `200 ok` |
| `GET /version` | `{"server_version":"0.1.0","protocol_version":1,"dataset_fingerprint":"9907ec8ae376fbf8"}` |

### Limits

| What | Limit | Result |
|---|---|---|
| Client frame size | 4096 bytes | `bad_message`, then the connection is closed (WebSocket code 1008) |
| Frame type | text only | binary frames get `bad_message` (connection kept) |
| Message rate per connection | 10 per second, bursts of 20; every frame counts, pongs included | `rate_limited` errors; a client that keeps flooding is closed (1008) |
| Request head | a client has 10 s to send a complete HTTP request head (and 16 KiB at most) | the connection is closed (slow-client / slowloris protection) |
| Sockets before the WebSocket upgrade | twice `BOUSSOLE_MAX_CONNECTIONS` | the extra socket is closed at once |
| Silent socket | no `hello` within 10 s | closed |
| Identified but in no room | no room joined or created within 60 s of `welcome` | closed (1008), so idle clients cannot occupy the connection limit |
| Idle socket | server pings every 20 s; **no frame at all (a pong is one) for 60 s** | dropped exactly then; the player keeps their slot for 60 s and can resume |
| Room-code guessing | more than 5 unknown room codes within a minute on one connection | closed (1008) |
| Slow reader | 64 queued messages, or a send not accepted within 10 s | dropped (same resume window) |
| Connections | `BOUSSOLE_MAX_CONNECTIONS` in all; `BOUSSOLE_MAX_CONNECTIONS_PER_IP` per client address | HTTP 503 / HTTP 429 |
| Rooms | `BOUSSOLE_MAX_ROOMS` | `create_room` answered `invalid_state` |
| Players per room | 2 to 8, chosen by the host | `room_full` |

The client address is the TCP peer's. Behind a tunnel or reverse proxy on the same machine
(loopback peer) the server uses the address the proxy reports (`Cf-Connecting-Ip`, or the last
`X-Forwarded-For` entry), so one visitor is not mistaken for everybody. A proxy on *another*
address (the compose `tls` profile, where Caddy is another container) is believed only with
`BOUSSOLE_TRUST_PROXY_HEADERS=true`: set it only if the server cannot be reached except through
that proxy, or clients could forge their address. The compose file sets it for the `tls` profile
only.

**Container networks that hide the visitor's address.** Rootless Podman (and Docker Desktop)
forward published ports through a helper, so every client then appears with the same address
(the container network's gateway) and the per-address limit applies to all of them together.
On such a setup raise it to the connection limit, for example
`BOUSSOLE_MAX_CONNECTIONS_PER_IP=2000` (the compose file passes it through). Docker on a Linux
server keeps real addresses. Caddy replaces any `X-Forwarded-For` a client sends with the
address it sees, so the header cannot be forged through it.

### Logs

Connections, disconnections (with the reason) and the number of rooms and players are logged at
`info`. Resume tokens, which are session credentials, and message contents are never logged.
`SIGTERM` and `Ctrl-C` shut down gracefully: the listener closes, clients receive a WebSocket
close (1001) and the process exits with status 0 once they have been told. If the engine task
ever dies on its own (a bug), `/healthz` answers `503 engine stopped` and the process exits
with a non-zero status so that `restart: unless-stopped` (or any supervisor) restarts it.

### Troubleshooting

| Symptom | Cause and fix |
|---|---|
| The game says the protocol is incompatible (`bad_version`) or the city list differs (`dataset_mismatch`) | Client and server were built from different versions of the repository. Update both from the same commit. `curl <server>/version` shows what the server runs. |
| Friends cannot connect on the LAN | Firewall prompt not accepted (macOS), another firewall, isolated guest Wi-Fi, wrong address (use `ws://`, port, and the path `/ws`). Test with `curl http://<ip>:7878/healthz` from their machine. |
| The tunnel does not start | `cloudflared` not installed or not on `PATH`; no internet; Cloudflare not answering within 20 s (try again). The message says which. |
| The tunnel address answers 502 (or a Cloudflare error page) | The tunnel was stopped (the address is dead for good) or lost its connection; restart and share the new address. |
| Behind a proxy, the WebSocket connects and drops, or never upgrades | The proxy must forward `Upgrade` / `Connection` headers over HTTP/1.1 and not time out idle connections under 30 s (the server pings every 20 s). Caddy does this by default. |
| Caddy exits with *"wrong argument count … after 'email'"* | `BOUSSOLE_DOMAIN` / `ACME_EMAIL` are not set in the shell or `.env` that runs `compose`. |
| No certificate | DNS does not point at the machine yet, or ports 80/443 are not reachable from the internet. `docker compose … logs caddy` shows the ACME error. |
| Certificate or TLS errors, strange timestamps | The machine's clock is wrong: enable time synchronisation (NTP). The game itself only uses the server's monotonic clock and durations, never the clients' clocks. |
| `create_room` is refused with *Action impossible à ce moment* | The server is at its room cap (`BOUSSOLE_MAX_ROOMS`). |
| HTTP 503 on `/ws` | Connection cap reached, or the server is shutting down. |
| HTTP 429 on `/ws` | Too many connections from your address (`BOUSSOLE_MAX_CONNECTIONS_PER_IP`; 4 when hosted from the game). Close your other clients, or use the LAN address instead of the tunnel. If *every* client is refused after about 20 connections, the container network hides client addresses (rootless Podman, Docker Desktop): see [Limits](#limits). |
| The server closes a connection that was idle in the menu | A connection must create or join a room within 60 s of connecting. Clients should connect when the player is ready to host or join, not earlier. |
| `/healthz` answers 503, or the container keeps restarting | The engine task died: this is a bug, `docker logs` shows the last lines (a panic message); report it. |
| Podman: *HEALTHCHECK is not supported for OCI image format* | Build with `podman build --format docker`; compose files carry their own health check anyway. |
