use anyhow::{bail, Context, Result};
use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    env, fs,
    io::Write,
    net::{IpAddr, ToSocketAddrs},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

const RUN: &str = "/run/v2engine";
const PID: &str = "/run/v2engine/sing-box.pid";
const BRIDGE_PID: &str = "/run/v2engine/tcp-http.pid";
const CONF: &str = "/run/v2engine/config.json";
const BIN: &str = "/usr/lib/v2engine/sing-box";
const HELPER_BIN: &str = "/usr/lib/v2engine/v2engine-helper";

#[path = "../tcp_http.rs"]
mod tcp_http;

#[derive(Deserialize)]
struct BridgeRequest {
    #[serde(rename = "type")]
    kind: String,
    listen_port: u16,
    server: String,
    server_port: u16,
    host: String,
    path: String,
}
fn pid() -> Option<i32> {
    fs::read_to_string(PID).ok()?.trim().parse().ok()
}
fn alive(p: i32) -> bool {
    kill(Pid::from_raw(p), None).is_ok()
}
fn is_ours(p: i32) -> bool {
    fs::read_link(format!("/proc/{p}/exe"))
        .ok()
        .and_then(|path| fs::canonicalize(path).ok())
        == fs::canonicalize(BIN).ok()
}

fn is_executable(p: i32, executable: &str) -> bool {
    fs::read_link(format!("/proc/{p}/exe"))
        .ok()
        .and_then(|path| fs::canonicalize(path).ok())
        == fs::canonicalize(executable).ok()
}

fn stop_bridge() {
    let bridge_pid = fs::read_to_string(BRIDGE_PID)
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok());
    if let Some(bridge_pid) = bridge_pid {
        if alive(bridge_pid) && is_executable(bridge_pid, HELPER_BIN) {
            let _ = kill(Pid::from_raw(-bridge_pid), Signal::SIGTERM);
            for _ in 0..20 {
                if !alive(bridge_pid) {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            if alive(bridge_pid) {
                let _ = kill(Pid::from_raw(-bridge_pid), Signal::SIGKILL);
            }
        }
    }
    let _ = fs::remove_file(BRIDGE_PID);
}

fn cleanup_network() {
    let _ = Command::new("nft")
        .args(["delete", "table", "inet", "sing-box"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = Command::new("ip")
        .args(["link", "delete", "v2engine0"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    for family in ["-4", "-6"] {
        let _ = Command::new("ip")
            .args([family, "route", "flush", "table", "20228"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        for priority in 9028..9040 {
            loop {
                let status = Command::new("ip")
                    .args([family, "rule", "delete", "priority", &priority.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                if !status.is_ok_and(|value| value.success()) {
                    break;
                }
            }
        }
    }
}

fn stop() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        bail!("root privileges required")
    }
    if let Some(p) = pid() {
        if alive(p) && is_ours(p) {
            let _ = kill(Pid::from_raw(-p), Signal::SIGTERM);
            for _ in 0..30 {
                if !alive(p) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            if alive(p) {
                let _ = kill(Pid::from_raw(-p), Signal::SIGKILL);
                for _ in 0..10 {
                    if !alive(p) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
    stop_bridge();
    cleanup_network();
    let _ = fs::remove_file(PID);
    let _ = fs::remove_file(CONF);
    Ok(())
}
fn source(path: &str) -> Result<PathBuf> {
    let uid: u32 = env::var("PKEXEC_UID")
        .context("must run through pkexec")?
        .parse()?;
    let expected = PathBuf::from(format!("/run/user/{uid}/v2engine/config.json"));
    let got = fs::canonicalize(path)?;
    if got != expected {
        bail!("invalid config path")
    }
    let m = fs::metadata(&got)?;
    if !m.is_file() || m.len() > 1024 * 1024 || m.uid() != uid || m.mode() & 0o077 != 0 {
        bail!("unsafe config permissions")
    }
    Ok(got)
}

fn only_keys(value: &Value, allowed: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|o| o.keys().all(|k| allowed.contains(&k.as_str())))
}

fn text_ok(value: Option<&Value>, max: usize) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty() && s.len() <= max)
}

fn validate_proxy(value: &Value) -> bool {
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return false;
    };
    let allowed = match kind {
        "vless" => &[
            "type",
            "tag",
            "server",
            "server_port",
            "uuid",
            "flow",
            "packet_encoding",
            "domain_resolver",
            "tls",
            "transport",
        ][..],
        "vmess" => &[
            "type",
            "tag",
            "server",
            "server_port",
            "uuid",
            "security",
            "alter_id",
            "packet_encoding",
            "domain_resolver",
            "tls",
            "transport",
        ][..],
        "trojan" => &[
            "type",
            "tag",
            "server",
            "server_port",
            "password",
            "domain_resolver",
            "tls",
            "transport",
        ][..],
        "shadowsocks" => &[
            "type",
            "tag",
            "server",
            "server_port",
            "method",
            "password",
            "domain_resolver",
        ][..],
        "ssh" => &[
            "type",
            "tag",
            "server",
            "server_port",
            "user",
            "password",
            "private_key",
            "private_key_passphrase",
            "domain_resolver",
        ][..],
        _ => return false,
    };
    if !only_keys(value, allowed)
        || value.get("tag").and_then(Value::as_str) != Some("proxy")
        || value.get("domain_resolver").and_then(Value::as_str) != Some("dns-direct")
        || !text_ok(value.get("server"), 253)
        || !value
            .get("server_port")
            .and_then(Value::as_u64)
            .is_some_and(|p| p > 0 && p <= 65535)
    {
        return false;
    }
    if value
        .get("packet_encoding")
        .is_some_and(|encoding| !matches!(encoding.as_str(), Some("packetaddr" | "xudp")))
        || value.get("alter_id").is_some_and(|alter_id| {
            alter_id
                .as_u64()
                .is_none_or(|value| value > u16::MAX as u64)
        })
    {
        return false;
    }
    if let Some(tls) = value.get("tls") {
        if !only_keys(
            tls,
            &[
                "enabled",
                "server_name",
                "insecure",
                "alpn",
                "reality",
                "utls",
            ],
        ) {
            return false;
        }
        if let Some(reality) = tls.get("reality") {
            if !only_keys(reality, &["enabled", "public_key", "short_id"]) {
                return false;
            }
        }
        if let Some(utls) = tls.get("utls") {
            if !only_keys(utls, &["enabled", "fingerprint"]) {
                return false;
            }
        }
        if tls.get("alpn").is_some_and(|alpn| {
            !alpn.as_array().is_some_and(|values| {
                !values.is_empty()
                    && values.len() <= 8
                    && values.iter().all(|value| text_ok(Some(value), 32))
            })
        }) {
            return false;
        }
    }
    if let Some(transport) = value.get("transport") {
        let Some(t) = transport.get("type").and_then(Value::as_str) else {
            return false;
        };
        let keys = match t {
            "ws" => &[
                "type",
                "path",
                "headers",
                "max_early_data",
                "early_data_header_name",
            ][..],
            "grpc" => &["type", "service_name"][..],
            "http" => &["type", "path", "host"][..],
            "httpupgrade" => &["type", "path", "host"][..],
            _ => return false,
        };
        if !only_keys(transport, keys) {
            return false;
        }
    }
    true
}

fn validate_policy(value: &Value) -> bool {
    if !only_keys(value, &["log", "dns", "inbounds", "outbounds", "route"]) {
        return false;
    }
    let Some(log) = value.get("log") else {
        return false;
    };
    if !only_keys(log, &["level", "timestamp"])
        || log.get("level").and_then(Value::as_str) != Some("warn")
    {
        return false;
    }
    let Some(dns) = value.get("dns") else {
        return false;
    };
    if !only_keys(dns, &["servers"]) {
        return false;
    }
    let Some(dns_servers) = dns.get("servers").and_then(Value::as_array) else {
        return false;
    };
    if dns_servers.len() != 2
        || !only_keys(&dns_servers[0], &["type", "tag"])
        || dns_servers[0].get("type").and_then(Value::as_str) != Some("local")
        || dns_servers[0].get("tag").and_then(Value::as_str) != Some("dns-direct")
        || !only_keys(&dns_servers[1], &["type", "tag", "server"])
        || dns_servers[1].get("type").and_then(Value::as_str) != Some("https")
        || dns_servers[1].get("tag").and_then(Value::as_str) != Some("dns-remote")
        || dns_servers[1].get("server").and_then(Value::as_str) != Some("1.1.1.1")
    {
        return false;
    }
    let Some(inbounds) = value.get("inbounds").and_then(Value::as_array) else {
        return false;
    };
    if inbounds.len() != 1
        || !only_keys(
            &inbounds[0],
            &[
                "type",
                "tag",
                "interface_name",
                "address",
                "mtu",
                "auto_route",
                "auto_redirect",
                "auto_redirect_output_mark",
                "strict_route",
                "iproute2_table_index",
                "iproute2_rule_index",
                "dns_mode",
                "route_exclude_address",
            ],
        )
        || inbounds[0].get("type").and_then(Value::as_str) != Some("tun")
        || inbounds[0].get("interface_name").and_then(Value::as_str) != Some("v2engine0")
        || inbounds[0]
            .get("address")
            .and_then(Value::as_array)
            .map(|a| a.as_slice())
            != Some(&[
                Value::String("172.19.0.1/30".into()),
                Value::String("fdfe:dcba:9876::1/126".into()),
            ])
        || inbounds[0].get("mtu").and_then(Value::as_u64) != Some(1500)
        || inbounds[0].get("auto_route").and_then(Value::as_bool) != Some(true)
        || inbounds[0].get("auto_redirect").and_then(Value::as_bool) != Some(true)
        || inbounds[0]
            .get("auto_redirect_output_mark")
            .and_then(Value::as_u64)
            != Some(8228)
        || inbounds[0].get("strict_route").and_then(Value::as_bool) != Some(true)
        || inbounds[0]
            .get("iproute2_table_index")
            .and_then(Value::as_u64)
            != Some(20228)
        || inbounds[0]
            .get("iproute2_rule_index")
            .and_then(Value::as_u64)
            != Some(9028)
        || inbounds[0].get("dns_mode").and_then(Value::as_str) != Some("hijack")
    {
        return false;
    }
    if inbounds[0]
        .get("route_exclude_address")
        .is_some_and(|addresses| {
            !addresses.as_array().is_some_and(|addresses| {
                !addresses.is_empty()
                    && addresses.len() <= 16
                    && addresses.iter().all(|address| {
                        address.as_str().is_some_and(|address| {
                            address
                                .strip_suffix("/32")
                                .and_then(|value| value.parse::<std::net::Ipv4Addr>().ok())
                                .is_some()
                                || address
                                    .strip_suffix("/128")
                                    .and_then(|value| value.parse::<std::net::Ipv6Addr>().ok())
                                    .is_some()
                        })
                    })
            })
        })
    {
        return false;
    }
    let Some(outbounds) = value.get("outbounds").and_then(Value::as_array) else {
        return false;
    };
    if outbounds.len() != 2
        || !validate_proxy(&outbounds[0])
        || !only_keys(&outbounds[1], &["type", "tag", "domain_resolver"])
        || outbounds[1].get("type").and_then(Value::as_str) != Some("direct")
        || outbounds[1].get("tag").and_then(Value::as_str) != Some("direct")
        || outbounds[1].get("domain_resolver").and_then(Value::as_str) != Some("dns-direct")
    {
        return false;
    }
    let Some(route) = value.get("route") else {
        return false;
    };
    if !only_keys(
        route,
        &[
            "rules",
            "final",
            "auto_detect_interface",
            "default_domain_resolver",
        ],
    ) || route.get("final").and_then(Value::as_str) != Some("proxy")
        || route.get("auto_detect_interface").and_then(Value::as_bool) != Some(true)
        || route.get("default_domain_resolver").and_then(Value::as_str) != Some("dns-remote")
    {
        return false;
    }
    let Some(rules) = route.get("rules").and_then(Value::as_array) else {
        return false;
    };
    let common = |rule: &Value| {
        only_keys(
            rule,
            &["ip_is_private", "domain_suffix", "action", "outbound"],
        ) && rule.get("action").and_then(Value::as_str) == Some("route")
            && rule.get("outbound").and_then(Value::as_str) == Some("direct")
    };
    if !(rules.len() == 1 || rules.len() == 2)
        || !common(&rules[0])
        || rules[0].get("ip_is_private").and_then(Value::as_bool) != Some(true)
        || rules[0].get("domain_suffix").is_some()
    {
        return false;
    }
    rules.get(1).is_none_or(|rule| {
        common(rule)
            && rule.get("ip_is_private").is_none()
            && rule
                .get("domain_suffix")
                .and_then(Value::as_array)
                .is_some_and(|domains| {
                    !domains.is_empty()
                        && domains.len() <= 1000
                        && domains.iter().all(|d| text_ok(Some(d), 253))
                })
    })
}

fn parse_bridge(value: Value) -> Result<BridgeRequest> {
    if !only_keys(
        &value,
        &[
            "type",
            "listen_port",
            "server",
            "server_port",
            "host",
            "path",
        ],
    ) {
        bail!("invalid bridge configuration")
    }
    let request: BridgeRequest = serde_json::from_value(value)?;
    if request.kind != "tcp-http"
        || request.listen_port == 0
        || request.server_port == 0
        || request.server.is_empty()
        || request.server.len() > 253
        || request.host.len() > 253
        || request.path.len() > 2048
    {
        bail!("invalid bridge configuration")
    }
    tcp_http::BridgeConfig::new(
        request.server.clone(),
        request.server_port,
        request.host.clone(),
        request.path.clone(),
    )?;
    Ok(request)
}

fn resolve_bridge(request: &BridgeRequest) -> Result<Vec<IpAddr>> {
    let mut addresses: Vec<_> = (request.server.as_str(), request.server_port)
        .to_socket_addrs()
        .context("cannot resolve TCP HTTP bridge server")?
        .map(|address| address.ip())
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() || addresses.len() > 16 {
        bail!("bridge server resolved to an invalid address set")
    }
    Ok(addresses)
}

fn add_bridge_route_exclusions(value: &mut Value, addresses: &[IpAddr]) -> Result<()> {
    let inbound = value
        .get_mut("inbounds")
        .and_then(Value::as_array_mut)
        .and_then(|inbounds| inbounds.first_mut())
        .ok_or_else(|| anyhow::anyhow!("missing TUN inbound"))?;
    inbound["route_exclude_address"] = Value::Array(
        addresses
            .iter()
            .map(|address| {
                Value::String(match address {
                    IpAddr::V4(address) => format!("{address}/32"),
                    IpAddr::V6(address) => format!("{address}/128"),
                })
            })
            .collect(),
    );
    Ok(())
}

fn start_bridge(request: &BridgeRequest, addresses: &[IpAddr]) -> Result<()> {
    let mut command = Command::new(HELPER_BIN);
    command
        .arg("bridge")
        .arg(request.listen_port.to_string())
        .arg(
            addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
        .arg(request.server_port.to_string())
        .arg(&request.host)
        .arg(&request.path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_clear();
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .context("cannot start TCP HTTP compatibility bridge")?;
    thread::sleep(Duration::from_millis(100));
    if let Some(status) = child.try_wait()? {
        bail!("TCP HTTP compatibility bridge exited during startup: {status}")
    }
    let save_pid = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o644)
            .open(BRIDGE_PID)?;
        writeln!(file, "{}", child.id())?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = save_pid {
        let _ = kill(Pid::from_raw(-(child.id() as i32)), Signal::SIGTERM);
        let _ = child.wait();
        return Err(error);
    }
    Ok(())
}

fn run_bridge(args: &[String]) -> Result<()> {
    if args.len() != 7 {
        bail!("invalid bridge invocation")
    }
    let listen_port = args[2].parse::<u16>()?;
    let server_port = args[4].parse::<u16>()?;
    if listen_port == 0 {
        bail!("invalid bridge listen port")
    }
    let config = tcp_http::BridgeConfig::new(
        args[3].clone(),
        server_port,
        args[5].clone(),
        args[6].clone(),
    )?;
    let listener = std::net::TcpListener::bind(("127.0.0.1", listen_port))?;
    tcp_http::serve(listener, config)
}

fn start(path: &str) -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        bail!("root privileges required")
    }
    let source = source(path)?;
    stop()?;
    let data = fs::read(&source)?;
    let mut value: Value = serde_json::from_slice(&data).context("invalid JSON")?;
    let bridge = value
        .as_object_mut()
        .and_then(|object| object.remove("v2engine_bridge"))
        .map(parse_bridge)
        .transpose()?;
    if value
        .get("inbounds")
        .and_then(Value::as_array)
        .and_then(|inbounds| inbounds.first())
        .and_then(|inbound| inbound.get("route_exclude_address"))
        .is_some()
    {
        bail!("route exclusions may only be generated by the helper")
    }
    let bridge_addresses = bridge.as_ref().map(resolve_bridge).transpose()?;
    if let Some(addresses) = &bridge_addresses {
        add_bridge_route_exclusions(&mut value, addresses)?;
    }
    if !validate_policy(&value) {
        bail!("config violates the privileged helper policy")
    }
    let data = serde_json::to_vec(&value)?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(RUN)?;
    fs::set_permissions(RUN, fs::Permissions::from_mode(0o755))?;
    let mut config_file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(CONF)?;
    config_file.write_all(&data)?;
    config_file.sync_all()?;
    fs::set_permissions(CONF, fs::Permissions::from_mode(0o600))?;
    if let (Some(bridge), Some(addresses)) = (&bridge, &bridge_addresses) {
        if let Err(error) = start_bridge(bridge, addresses) {
            let _ = fs::remove_file(CONF);
            return Err(error);
        }
    }
    let check = Command::new(BIN)
        .args(["check", "-c", CONF])
        .env_clear()
        .status();
    let check = match check {
        Ok(status) => status,
        Err(error) => {
            stop_bridge();
            let _ = fs::remove_file(CONF);
            return Err(error).context("cannot validate sing-box config");
        }
    };
    if !check.success() {
        stop_bridge();
        let _ = fs::remove_file(CONF);
        bail!("sing-box rejected the config")
    }
    let mut command = Command::new(BIN);
    command
        .args(["run", "-c", CONF])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_clear();
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            stop_bridge();
            let _ = fs::remove_file(CONF);
            return Err(error).context("cannot start sing-box");
        }
    };
    let mut ready = false;
    for _ in 0..50 {
        if let Some(status) = child.try_wait()? {
            cleanup_network();
            stop_bridge();
            let _ = fs::remove_file(CONF);
            bail!("sing-box exited during startup: {status}")
        }
        if std::path::Path::new("/sys/class/net/v2engine0").exists() {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if !ready {
        let _ = kill(Pid::from_raw(-(child.id() as i32)), Signal::SIGTERM);
        let _ = child.wait();
        cleanup_network();
        stop_bridge();
        let _ = fs::remove_file(CONF);
        bail!("TUN interface did not become ready")
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o644)
        .open(PID)?;
    writeln!(f, "{}", child.id())?;
    f.sync_all()?;
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("start") if args.len() == 3 => start(&args[2]),
        Some("stop") if args.len() == 2 => stop(),
        Some("status") if args.len() == 2 => {
            if pid().is_some_and(alive) {
                Ok(())
            } else {
                bail_result("not running")
            }
        }
        Some("bridge") => run_bridge(&args),
        _ => bail_result("usage: v2engine-helper start CONFIG|stop|status"),
    };
    if let Err(error) = result {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn bail_result(message: &str) -> Result<()> {
    bail!(message.to_string())
}
