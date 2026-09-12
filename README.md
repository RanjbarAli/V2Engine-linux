# V2Engine

<p align="center">
  <img src="packaging/io.github.ranjbarali.V2Engine.png" width="128" alt="V2Engine icon">
</p>

V2Engine is a lightweight native Linux proxy client built with Rust and GTK4. It uses a bundled sing-box core and a system-wide TUN interface while keeping the graphical application unprivileged.

## Features

- Native GTK4 interface with a compact charcoal and emerald design
- System-wide TUN routing powered by sing-box
- VLESS, VLESS Reality, VMess, Trojan, Shadowsocks and SSH support
- SSH password and private-key authentication
- Multiple newline-separated configuration imports
- Real proxied connectivity and latency tests with limited concurrency
- Direct Sites routing for domains and all their subdomains
- Native Linux status notifier with server selection and connection controls
- In-app update checks using GitHub Releases
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
| VLESS | `vless://` | TLS, Reality, WebSocket, gRPC, HTTP and HTTP Upgrade |
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

**Test All** starts temporary unprivileged sing-box instances and measures actual proxied TCP connectivity. Tests run concurrently with a limit of four workers, update each server progressively and do not transfer large amounts of data.

## Security and recovery

- Server configurations are stored in `~/.config/v2engine/servers.json` with mode `0600`.
- Configuration and runtime directories use mode `0700`.
- Passwords and private keys are never written to application logs.
- Runtime configurations use mode `0600` and are removed after handoff.
- The root helper accepts only `start`, `stop` and `status` operations.
- Runtime paths, ownership and permissions are validated before sing-box starts.
- No user input is passed through `sh -c` or unsafe shell construction.
- Repeated connections safely replace the owned process without signaling unrelated processes.
- sing-box cleans up TUN routes, nftables rules and DNS interception on disconnect.

Closing the window keeps the lightweight status notifier available. Selecting **Quit** stops an active connection before terminating V2Engine.

## Updates

The Settings screen checks the latest stable release from this repository. An update is accepted only when the expected amd64 `.deb` and its matching SHA-256 asset are both present. The downloaded package is verified before installation and installed through the system authentication prompt.

## Building from source

Ubuntu/Debian build dependencies include Rust, Cargo, `libgtk-4-dev`, `pkg-config`, `curl` and standard Debian packaging tools.

```bash
./packaging/fetch-sing-box.sh
cargo build --release --locked
./packaging/build-deb.sh
```

The finished package is written to `dist/V2Engine_1.0.0_amd64.deb`. The bundled sing-box version is pinned and checksum-verified by the fetch script.

## License

V2Engine is licensed under [GPL-3.0-or-later](LICENSE). The bundled sing-box binary is distributed under its upstream GPL-3.0-or-later license.

Created by [Ali Ranjbar Jelodar](https://github.com/RanjbarAli).
