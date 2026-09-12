use anyhow::{bail, Context, Result};
use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use serde_json::Value;
use std::{
    env, fs,
    io::Write,
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
const CONF: &str = "/run/v2engine/config.json";
const BIN: &str = "/usr/lib/v2engine/sing-box";
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
fn stop() -> Result<()> {
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
            }
        }
    }
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
            "tls",
            "transport",
        ][..],
        "trojan" => &[
            "type",
            "tag",
            "server",
            "server_port",
            "password",
            "tls",
            "transport",
        ][..],
        "shadowsocks" => &["type", "tag", "server", "server_port", "method", "password"][..],
        "ssh" => &[
            "type",
            "tag",
            "server",
            "server_port",
            "user",
            "password",
            "private_key",
            "private_key_passphrase",
        ][..],
        _ => return false,
    };
    if !only_keys(value, allowed)
        || value.get("tag").and_then(Value::as_str) != Some("proxy")
        || !text_ok(value.get("server"), 253)
        || !value
            .get("server_port")
            .and_then(Value::as_u64)
            .is_some_and(|p| p > 0 && p <= 65535)
    {
        return false;
    }
    if let Some(tls) = value.get("tls") {
        if !only_keys(
            tls,
            &["enabled", "server_name", "insecure", "reality", "utls"],
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
    }
    if let Some(transport) = value.get("transport") {
        let Some(t) = transport.get("type").and_then(Value::as_str) else {
            return false;
        };
        let keys = match t {
            "ws" => &["type", "path", "headers"][..],
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
    if dns_servers.len() != 1
        || !only_keys(&dns_servers[0], &["type", "tag", "server"])
        || dns_servers[0].get("type").and_then(Value::as_str) != Some("https")
        || dns_servers[0].get("tag").and_then(Value::as_str) != Some("dns-remote")
        || dns_servers[0].get("server").and_then(Value::as_str) != Some("1.1.1.1")
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
                "strict_route",
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
        || inbounds[0].get("mtu").and_then(Value::as_u64) != Some(9000)
        || inbounds[0].get("auto_route").and_then(Value::as_bool) != Some(true)
        || inbounds[0].get("auto_redirect").and_then(Value::as_bool) != Some(true)
        || inbounds[0].get("strict_route").and_then(Value::as_bool) != Some(true)
    {
        return false;
    }
    let Some(outbounds) = value.get("outbounds").and_then(Value::as_array) else {
        return false;
    };
    if outbounds.len() != 2
        || !validate_proxy(&outbounds[0])
        || !only_keys(&outbounds[1], &["type", "tag"])
        || outbounds[1].get("type").and_then(Value::as_str) != Some("direct")
        || outbounds[1].get("tag").and_then(Value::as_str) != Some("direct")
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

fn start(path: &str) -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        bail!("root privileges required")
    }
    let source = source(path)?;
    stop()?;
    let data = fs::read(&source)?;
    let value: Value = serde_json::from_slice(&data).context("invalid JSON")?;
    if !validate_policy(&value) {
        bail!("config violates the privileged helper policy")
    }
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
    let check = Command::new(BIN)
        .args(["check", "-c", CONF])
        .env_clear()
        .status()
        .context("cannot validate sing-box config")?;
    if !check.success() {
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
    let mut child = command.spawn().context("cannot start sing-box")?;
    thread::sleep(Duration::from_millis(700));
    if let Some(status) = child.try_wait()? {
        let _ = fs::remove_file(CONF);
        bail!("sing-box exited during startup: {status}")
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
