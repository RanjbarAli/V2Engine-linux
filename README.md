# V2Engine

<p align="center">
  <img src="packaging/io.github.ranjbarali.V2Engine.png" width="128" alt="V2Engine icon">
</p>

V2Engine is a lightweight native Linux proxy client built with Rust and GTK4. It uses a bundled sing-box core and a system-wide TUN interface while keeping the graphical application unprivileged.

## Features

- Native GTK4 interface with a compact charcoal and emerald design
- Three in-window pages for Servers, Direct Sites and Settings
- System-wide TUN routing powered by sing-box
- VLESS, VLESS Reality, VMess, Trojan, Shadowsocks and SSH support
- Compatibility for legacy VLESS/VMess TCP HTTP-header camouflage links
- SSH password and private-key authentication
- Multi-server clipboard import from newline-, whitespace- or text-separated links
- Real proxied connectivity and latency tests with limited concurrency
- Direct Sites routing for domains and all their subdomains
- Native Linux status notifier with server selection and connection controls
- In-app update checks using GitHub Releases
- Live public IP, download/upload speed and transferred-byte counters
- Optional desktop-session startup
- No Electron, WebView, database or background polling

## Installation

After adding the V2Engine package source, install the latest release with:

```bash
sudo apt update
sudo apt install v2engine
```

The package contains V2Engine, the compatible sing-box binary, desktop entry, application icon and Polkit policy. No separate core or service installation is required.

## Quick start

1. Open **V2Engine** from the application menu.
2. Copy one or more supported server configurations.
3. Press `Ctrl+V` or select **Add Server**.
4. Select a server and press the power button.

The GTK application always runs as the current user. A system authentication prompt appears only when a privileged networking operation or package update is required.

## Supported configurations

| Protocol | Import format | Notes |
| --- | --- | --- |
| VLESS | `vless://` | TLS, Reality, TCP HTTP-header camouflage, WebSocket, gRPC, HTTP and HTTP Upgrade |
| VMess | `vmess://` | Base64 JSON links |
| Trojan | `trojan://` | TLS and supported transports |
| Shadowsocks | `ss://` | SIP002 and legacy Base64 links |
| SSH | `ssh://` | Password or embedded private key |

SSH password example:

```text
ssh://user:password@example.com:22#MyServer
```

SSH private-key example:

```text
ssh://user@example.com:22?privateKey=BASE64_KEY&passphrase=OPTIONAL#MyServer
```

External Shadowsocks plugins are intentionally rejected. Private keys must be encoded as URL-safe Base64 so the privileged core does not need access to files in the user's home directory.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+V` | Import configurations from the clipboard |
| `Ctrl+C` | Copy the selected server configuration |
| `Delete` | Delete the selected server |

## Direct Sites

Domains listed under **Direct Sites** bypass the proxy and use the normal connection. Adding `google.com` also covers every subdomain such as `mail.google.com`. V2Engine implements this through sing-box routing and never modifies `/etc/hosts`.

## Server testing

**Test All** waits for each temporary unprivileged sing-box instance to become ready, then measures real proxied HTTP/HTTPS requests against several small connectivity endpoints with retry fallback. Tests run concurrently with a limit of four workers, update each server progressively and do not transfer large amounts of data. If V2Engine is connected, it first stops that session so every server is measured directly rather than through the active proxy. Successful sub-millisecond measurements are displayed as at least `1 ms`, never `0 ms`.

## Security and recovery

- Server configurations are stored in `~/.config/v2engine/servers.json` with mode `0600`.
- Configuration and runtime directories use mode `0700`.
- Passwords and private keys are never written to application logs.
- Runtime configurations use mode `0600` and are removed after handoff.
- The root helper exposes only validated networking operations; the TCP HTTP-header compatibility bridge uses a fixed routing mark solely to keep its upstream socket outside the TUN loop.
- Runtime paths, ownership and permissions are validated before sing-box starts.
- No user input is passed through `sh -c` or unsafe shell construction.
- Repeated connections safely replace the owned process without signaling unrelated processes.
- A connection is shown as successful only after the TUN interface is ready and one of several lightweight connectivity checks succeeds through it. Public-IP lookup is independent, so a blocked IP service cannot incorrectly fail an otherwise healthy connection.
- Disconnect and tray Stop terminate the owned process, remove the TUN interface and flush V2Engine's dedicated routing table/rules.

Closing the window keeps the lightweight status notifier available. Selecting **Quit** stops an active connection before terminating V2Engine.

## Updates

The Settings screen checks the latest stable release from this repository. An update is accepted only when the expected amd64 `.deb` and its matching SHA-256 asset are both present. The downloaded package is verified before installation and installed through the system authentication prompt.

## Building from source

Ubuntu/Debian build dependencies include Rust, Cargo, Go 1.25.5, `libgtk-4-dev`, `pkg-config`, `curl` and standard Debian packaging tools.

```bash
./packaging/build-sing-box.sh
cargo build --release --locked
./packaging/build-deb.sh
```

The finished package is written to `dist/V2Engine_1.0.0_amd64.deb`. The build script verifies the pinned sing-box source archive, enables only the uTLS feature needed by V2Engine's supported protocols, and produces a smaller stripped core.

## License

V2Engine is licensed under [GPL-3.0-or-later](LICENSE). The bundled sing-box binary is distributed under its upstream GPL-3.0-or-later license.

Creator: Ali Ranjbar Jelodar · [V2Engine repository](https://github.com/RanjbarAli/V2Engine-linux)
