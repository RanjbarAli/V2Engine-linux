use anyhow::{anyhow, bail, Context, Result};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub uri: String,
    #[serde(skip)]
    pub latency: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Store {
    pub servers: Vec<Server>,
    pub selected: Option<String>,
    pub bypass: Vec<String>,
}

pub fn config_dir() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .ok_or_else(|| anyhow!("No configuration directory"))?
        .join("v2engine"))
}

pub fn load() -> Store {
    config_dir()
        .ok()
        .and_then(|p| fs::read(p.join("servers.json")).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(store: &Store) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let dir = config_dir()?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    let path = dir.join("servers.json");
    let tmp = dir.join("servers.json.tmp");
    let mut f = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)?;
    serde_json::to_writer_pretty(&mut f, store)?;
    f.sync_all()?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn decode_b64(s: &str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(s.trim())
        .or_else(|_| STANDARD.decode(s.trim()))
        .context("invalid base64")
}

fn id_for(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

pub fn parse_many(text: &str) -> (Vec<Server>, Vec<String>) {
    let mut ok = vec![];
    let mut bad = vec![];
    for line in text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        match parse(line) {
            Ok(s) => ok.push(s),
            Err(e) => bad.push(format!(
                "{}: {}",
                line.split(':').next().unwrap_or("config"),
                e
            )),
        }
    }
    (ok, bad)
}

pub fn parse(raw: &str) -> Result<Server> {
    let scheme = raw.split(':').next().unwrap_or("").to_ascii_lowercase();
    if !matches!(scheme.as_str(), "vless" | "vmess" | "trojan" | "ss" | "ssh") {
        bail!("unsupported protocol")
    }
    let name = if scheme == "vmess" {
        let v: Value = serde_json::from_slice(&decode_b64(raw.trim_start_matches("vmess://"))?)?;
        v.get("ps")
            .and_then(Value::as_str)
            .unwrap_or("VMess server")
            .to_string()
    } else {
        let u = Url::parse(raw).context("invalid URI")?;
        percent_encoding::percent_decode_str(u.fragment().unwrap_or(""))
            .decode_utf8_lossy()
            .trim()
            .to_string()
    };
    let name = if name.is_empty() {
        format!("{} server", scheme.to_uppercase())
    } else {
        name
    };
    let server = Server {
        id: id_for(raw),
        name,
        protocol: if scheme == "ss" {
            "Shadowsocks".into()
        } else {
            let mut c = scheme;
            if let Some(x) = c.get_mut(0..1) {
                x.make_ascii_uppercase();
            }
            c
        },
        uri: raw.to_string(),
        latency: None,
    };
    outbound(&server)?;
    Ok(server)
}

fn qp(u: &Url, key: &str) -> Option<String> {
    u.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .filter(|s| !s.is_empty())
}
fn host_port(u: &Url, default: u16) -> Result<(String, u16)> {
    Ok((
        u.host_str()
            .ok_or_else(|| anyhow!("missing server"))?
            .to_string(),
        u.port().unwrap_or(default),
    ))
}

fn valid_uuid(value: &str) -> bool {
    let compact = value.replace('-', "");
    compact.len() == 32 && compact.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn outbound(s: &Server) -> Result<Value> {
    match s
        .uri
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "vmess" => vmess(&s.uri),
        "ss" => shadowsocks(&s.uri),
        "vless" | "trojan" => url_outbound(&s.uri),
        "ssh" => ssh(&s.uri),
        _ => bail!("unsupported protocol"),
    }
}

fn vmess(raw: &str) -> Result<Value> {
    let v: Value = serde_json::from_slice(&decode_b64(raw.trim_start_matches("vmess://"))?)?;
    let server = v["add"].as_str().ok_or_else(|| anyhow!("missing server"))?;
    let port = v["port"]
        .as_str()
        .and_then(|p| p.parse().ok())
        .or_else(|| v["port"].as_u64().map(|p| p as u16))
        .ok_or_else(|| anyhow!("missing port"))?;
    let uuid = v["id"].as_str().ok_or_else(|| anyhow!("missing UUID"))?;
    if !valid_uuid(uuid) {
        bail!("invalid UUID")
    }
    let mut o = json!({"type":"vmess","tag":"proxy","server":server,"server_port":port,"uuid":uuid,"security":v["scy"].as_str().unwrap_or("auto")});
    if v["tls"].as_str().unwrap_or("") == "tls" {
        o["tls"] = json!({"enabled":true,"server_name":v["sni"].as_str().unwrap_or(server),"insecure":v["allowInsecure"].as_bool().unwrap_or(false)});
    }
    let network = v["net"].as_str().unwrap_or("tcp");
    let transport_kind = if network == "tcp" && v["type"].as_str() == Some("http") {
        "http"
    } else {
        network
    };
    transport(
        &mut o,
        transport_kind,
        v["host"].as_str(),
        v["path"].as_str(),
        v["type"].as_str(),
    );
    Ok(o)
}

fn url_outbound(raw: &str) -> Result<Value> {
    let u = Url::parse(raw)?;
    let kind = u.scheme();
    let (host, port) = host_port(&u, 443)?;
    let secret = percent_encoding::percent_decode_str(u.username())
        .decode_utf8_lossy()
        .to_string();
    if secret.is_empty() {
        bail!("missing credential")
    }
    if kind == "vless" && !valid_uuid(&secret) {
        bail!("invalid UUID")
    }
    let mut o = if kind == "vless" {
        json!({"type":"vless","tag":"proxy","server":host,"server_port":port,"uuid":secret})
    } else {
        json!({"type":"trojan","tag":"proxy","server":host,"server_port":port,"password":secret})
    };
    if let Some(flow) = qp(&u, "flow") {
        o["flow"] = json!(flow)
    }
    let security = qp(&u, "security").unwrap_or_default();
    if security == "tls" || security == "reality" {
        let mut tls = json!({"enabled":true,"server_name":qp(&u,"sni").unwrap_or(host.clone()),"insecure":qp(&u,"allowInsecure").as_deref()==Some("1")});
        if security == "reality" {
            let pk = qp(&u, "pbk").ok_or_else(|| anyhow!("Reality public key missing"))?;
            tls["reality"] =
                json!({"enabled":true,"public_key":pk,"short_id":qp(&u,"sid").unwrap_or_default()});
        }
        if let Some(fp) = qp(&u, "fp") {
            tls["utls"] = json!({"enabled":true,"fingerprint":fp});
        }
        o["tls"] = tls;
    }
    let network = qp(&u, "type").unwrap_or_else(|| "tcp".into());
    let transport_kind = if network == "tcp" && qp(&u, "headerType").as_deref() == Some("http") {
        "http"
    } else {
        &network
    };
    transport(
        &mut o,
        transport_kind,
        qp(&u, "host").as_deref(),
        qp(&u, "path").as_deref(),
        qp(&u, "serviceName").as_deref(),
    );
    Ok(o)
}

fn transport(
    o: &mut Value,
    kind: &str,
    host: Option<&str>,
    path: Option<&str>,
    service: Option<&str>,
) {
    match kind {
        "ws" => {
            o["transport"] = json!({"type":"ws","path":path.unwrap_or("/"),"headers":if let Some(h)=host {json!({"Host":h})} else {json!({})}})
        }
        "grpc" => o["transport"] = json!({"type":"grpc","service_name":service.unwrap_or("")}),
        "http" => {
            o["transport"] = json!({"type":"http","path":path.unwrap_or("/"),"host":host.map(|h|vec![h]).unwrap_or_default()})
        }
        "httpupgrade" => {
            o["transport"] =
                json!({"type":"httpupgrade","path":path.unwrap_or("/"),"host":host.unwrap_or("")})
        }
        _ => {}
    }
}

fn shadowsocks(raw: &str) -> Result<Value> {
    let payload = raw.trim_start_matches("ss://");
    if !payload.contains('@') {
        let fragment = payload.split('#').next().unwrap_or(payload);
        let decoded = String::from_utf8(decode_b64(fragment)?)?;
        return shadowsocks(&format!("ss://{decoded}"));
    }
    let u = Url::parse(raw)?;
    if qp(&u, "plugin").is_some() {
        bail!("Shadowsocks plugins are not supported")
    }
    let (host, port) = host_port(&u, 8388)?;
    let auth = if !u.username().is_empty() {
        percent_encoding::percent_decode_str(u.username())
            .decode_utf8_lossy()
            .to_string()
    } else {
        bail!("missing Shadowsocks credentials")
    };
    let auth = if auth.contains(':') {
        auth
    } else {
        String::from_utf8(decode_b64(&auth)?)?
    };
    let (method, password) = auth
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid Shadowsocks credentials"))?;
    if method.is_empty() || password.is_empty() {
        bail!("invalid Shadowsocks credentials")
    }
    Ok(
        json!({"type":"shadowsocks","tag":"proxy","server":host,"server_port":port,"method":method,"password":password}),
    )
}

fn ssh(raw: &str) -> Result<Value> {
    let u = Url::parse(raw)?;
    let (host, port) = host_port(&u, 22)?;
    let user = percent_encoding::percent_decode_str(u.username())
        .decode_utf8_lossy()
        .to_string();
    if user.is_empty() {
        bail!("missing SSH user")
    }
    let mut o = json!({"type":"ssh","tag":"proxy","server":host,"server_port":port,"user":user});
    if let Some(p) = u.password() {
        o["password"] = json!(percent_encoding::percent_decode_str(p).decode_utf8_lossy())
    }
    if let Some(k) = qp(&u, "privateKey").or_else(|| qp(&u, "private_key")) {
        o["private_key"] = json!(String::from_utf8(decode_b64(&k)?).context("invalid private key")?)
    }
    if let Some(p) = qp(&u, "passphrase") {
        o["private_key_passphrase"] = json!(p)
    }
    if o.get("password").is_none() && o.get("private_key").is_none() {
        bail!("SSH password or privateKey is required")
    }
    Ok(o)
}

pub fn singbox_config(
    server: &Server,
    bypass: &[String],
    tun: bool,
    socks_port: Option<u16>,
) -> Result<Value> {
    let mut proxy = outbound(server)?;
    proxy["tag"] = json!("proxy");
    let inbound = if tun {
        json!({"type":"tun","tag":"tun-in","interface_name":"v2engine0","address":["172.19.0.1/30","fdfe:dcba:9876::1/126"],"mtu":1500,"auto_route":true,"auto_redirect":true,"strict_route":true,"iproute2_table_index":20228,"iproute2_rule_index":9028,"dns_mode":"hijack"})
    } else {
        json!({"type":"mixed","tag":"test-in","listen":"127.0.0.1","listen_port":socks_port.unwrap_or(19090)})
    };
    let domains: Vec<_> = bypass
        .iter()
        .map(|d| {
            d.trim()
                .trim_start_matches("*.")
                .trim_start_matches('.')
                .to_ascii_lowercase()
        })
        .filter(|d| !d.is_empty())
        .collect();
    let mut rules = vec![json!({"ip_is_private":true,"action":"route","outbound":"direct"})];
    if !domains.is_empty() {
        rules.push(json!({"domain_suffix":domains,"action":"route","outbound":"direct"}));
    }
    Ok(
        json!({"log":{"level":"warn","timestamp":true},"dns":{"servers":[{"type":"https","tag":"dns-remote","server":"1.1.1.1"}]},"inbounds":[inbound],"outbounds":[proxy,{"type":"direct","tag":"direct"}],"route":{"rules":rules,"final":"proxy","auto_detect_interface":true,"default_domain_resolver":"dns-remote"}}),
    )
}

pub fn valid_domain(d: &str) -> bool {
    let d = d.trim().trim_start_matches("*.").trim_start_matches('.');
    d.len() <= 253
        && !d.is_empty()
        && d.split('.').all(|p| {
            !p.is_empty()
                && p.len() <= 63
                && !p.starts_with('-')
                && !p.ends_with('-')
                && p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}
