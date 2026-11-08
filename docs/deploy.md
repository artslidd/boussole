# Running a Boussole server

The multiplayer server is one small binary, `boussole-server` (axum + WebSocket). It keeps
everything in memory: no database, no accounts, nothing to back up. There are four ways to
run it, from "no setup at all" to "always on":

| | Who | Needs | Players connect to |
|---|---|---|---|
| [0. From a player's computer](#0-free-play-from-a-players-computer-recommended) | one player hosts, friends anywhere | the game (or `cargo`) + free `cloudflared` | `wss://<random-words>.trycloudflare.com/ws` |
| [1. LAN](#1-lan-play) | people on the same network | the game (or `cargo`) | `ws://<host-ip>:7878/ws` |
| [2. One container](#2-docker--podman-single-container) | a machine that already runs containers | Docker or Podman | `ws://<host-ip>:7878/ws` |
| [3. Always-on server](#3-optional-always-on-remote-host) | a small VPS | Docker/Podman + a domain name | `wss://<your-domain>/ws` |

The wire protocol is specified in [`protocol.md`](protocol.md). Player-facing text in the game
is French; this guide is for whoever runs the server.

## 0. Free play from a player's computer (recommended)

One player runs the server on their own computer and shares an address; nobody pays for a
server and nobody needs an account.

### From the game

**Héberger une partie** (in the game's online menu) starts this same server *inside the app*
(`boussole_server::start`) and shows the addresses to share: the LAN address, and, if you ask
for it, the public tunnel address below. Friends choose **Rejoindre** and enter the address
and the room code.

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
  friends each time.
- **Testing grade:** Cloudflare gives quick tunnels no uptime guarantee (and limits concurrent
  requests). It is meant for game nights, not for a permanent service. For that, use
  [section 3](#3-optional-always-on-remote-host).
- **Anyone who has the address can reach your server.** What protects a game is the room: a
  random 5-letter code you only share with friends, a cap of 8 players, and the server's
  limits (frame size, message rate, connection and room caps; see the [reference](#limits)).
  The server exposes nothing but the game, `/healthz` and `/version`. Do not keep the tunnel
  open longer than you play.
- The traffic between your friends and Cloudflare is encrypted (`wss://`); Cloudflare can see it.
  There is nothing secret in a game of Boussole, but you should know.
- `--tunnel` passes `cloudflared` an empty configuration file: a `cloudflared` set up for
  something else on the same machine (a named tunnel, `proxy-dns`, ...) would otherwise make
  quick tunnels refuse to start (*"running a quick tunnel with proxy-dns is not supported"*).
  The program that is already running keeps running; it is never touched.
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
- Behind Caddy, the server sees connections coming from Caddy's container address, so its logs
  do not show players' own addresses (Caddy's logs do).
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
| `RUST_LOG` | `info` | log filter, e.g. `debug`, `boussole_server=debug` |
| `BOUSSOLE_LOG_FORMAT` | text | `json` for one JSON object per log line |

Malformed values stop the server at startup with a message (exit code 2). Flags: `--tunnel`,
`--healthcheck` (exit 0 if `/healthz` answers 200 on the configured port; used by the
container), `--help`.

When the server is embedded in the game (*Héberger une partie*), it uses small limits instead
(16 connections, 4 rooms), chosen by `ServerConfig::for_host_mode`.

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
| Message rate per connection | 10 per second, bursts of 20 | `rate_limited` errors; a client that keeps flooding is closed (1008) |
| Silent socket | no `hello` within 10 s | closed |
| Idle socket | server pings every 20 s; nothing (not even a pong) for 60 s | dropped; the player keeps their slot for 60 s and can resume |
| Slow reader | 64 queued messages | dropped (same resume window) |
| Connections / rooms | `BOUSSOLE_MAX_CONNECTIONS` / `BOUSSOLE_MAX_ROOMS` | HTTP 503 / `invalid_state` |
| Players per room | 2 to 8, chosen by the host | `room_full` |

### Logs

Connections, disconnections (with the reason) and the number of rooms and players are logged at
`info`. Resume tokens, which are session credentials, and message contents are never logged.
`SIGTERM` and `Ctrl-C` shut down gracefully: clients receive a WebSocket close (1001) and the
process exits with status 0.

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
| Podman: *HEALTHCHECK is not supported for OCI image format* | Build with `podman build --format docker`; compose files carry their own health check anyway. |
