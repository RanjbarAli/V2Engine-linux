use anyhow::{bail, Context, Result};
use std::{
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

const MAX_HEADER: usize = 8192;
const MAX_CONNECTIONS: usize = 128;
const TUN_BYPASS_MARK: libc::c_uint = 0x2024;

#[derive(Clone)]
pub struct BridgeConfig {
    servers: Vec<String>,
    server_port: u16,
    host: String,
    path: String,
}

impl BridgeConfig {
    pub fn new(server: String, server_port: u16, host: String, path: String) -> Result<Self> {
        let servers: Vec<_> = server
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        if servers.is_empty()
            || servers.len() > 16
            || servers
                .iter()
                .any(|server| server.len() > 253 || server.contains(['\r', '\n', '\0']))
            || server_port == 0
        {
            bail!("invalid bridge server")
        }
        let host = host.split(',').next().unwrap_or("").trim().to_owned();
        if host.len() > 253 || host.contains(['\r', '\n', '\0']) {
            bail!("invalid HTTP camouflage host")
        }
        let path = path.split(',').next().unwrap_or("/").trim();
        let path = if path.is_empty() {
            "/".to_owned()
        } else if path.starts_with('/') {
            path.to_owned()
        } else {
            format!("/{path}")
        };
        if path.len() > 2048 || path.contains(['\r', '\n', '\0']) {
            bail!("invalid HTTP camouflage path")
        }
        Ok(Self {
            servers,
            server_port,
            host,
            path,
        })
    }
}

struct ConnectionGuard(Arc<AtomicUsize>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

pub fn serve(listener: TcpListener, config: BridgeConfig) -> Result<()> {
    let active = Arc::new(AtomicUsize::new(0));
    for local in listener.incoming() {
        let local = match local {
            Ok(local) => local,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        if active.fetch_add(1, Ordering::Relaxed) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::Relaxed);
            let _ = local.shutdown(Shutdown::Both);
            continue;
        }
        let guard = ConnectionGuard(active.clone());
        let config = config.clone();
        thread::spawn(move || {
            let _guard = guard;
            if let Err(error) = bridge_connection(local, &config) {
                if std::env::var_os("V2ENGINE_BRIDGE_DEBUG").is_some() {
                    eprintln!("TCP HTTP bridge connection failed: {error:#}");
                }
            }
        });
    }
    Ok(())
}

fn connect_remote(config: &BridgeConfig) -> Result<TcpStream> {
    let mut last_error = None;
    for server in &config.servers {
        let addresses = (server.as_str(), config.server_port)
            .to_socket_addrs()
            .context("cannot resolve bridge server")?;
        for address in addresses {
            let connection = if unsafe { libc::geteuid() } == 0 {
                let socket = socket2::Socket::new(
                    socket2::Domain::for_address(address),
                    socket2::Type::STREAM,
                    Some(socket2::Protocol::TCP),
                )?;
                socket.set_mark(TUN_BYPASS_MARK)?;
                socket
                    .connect_timeout(&address.into(), Duration::from_secs(8))
                    .map(|_| socket.into())
            } else {
                TcpStream::connect_timeout(&address, Duration::from_secs(8))
            };
            match connection {
                Ok(stream) => {
                    return Ok(stream);
                }
                Err(error) => last_error = Some(error),
            }
        }
    }
    Err(last_error
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow::anyhow!("bridge server resolved to no addresses")))
}

fn bridge_connection(mut local: TcpStream, config: &BridgeConfig) -> Result<()> {
    local.set_nodelay(true)?;
    let mut remote = connect_remote(config)?;
    remote.set_nodelay(true)?;
    remote.set_read_timeout(Some(Duration::from_secs(10)))?;

    write!(remote, "GET {} HTTP/1.1\r\n", config.path)?;
    if !config.host.is_empty() {
        write!(remote, "Host: {}\r\n", config.host)?;
    }
    remote.write_all(b"User-Agent: Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.6478.122 Mobile Safari/537.36\r\nAccept-Encoding: gzip, deflate\r\nConnection: keep-alive\r\nPragma: no-cache\r\n\r\n")?;

    let mut remote_reader = remote.try_clone()?;
    let mut local_writer = local.try_clone()?;
    let upstream = thread::spawn(move || {
        let result = std::io::copy(&mut local, &mut remote);
        let _ = remote.shutdown(Shutdown::Both);
        result
    });

    let mut response = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    while response.len() < MAX_HEADER {
        remote_reader.read_exact(&mut byte)?;
        response.push(byte[0]);
        if response.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    if !response.ends_with(b"\r\n\r\n") {
        bail!("HTTP camouflage response header is too large")
    }
    if std::env::var_os("V2ENGINE_BRIDGE_DEBUG").is_some() {
        let status = response.split(|byte| *byte == b'\n').next().unwrap_or(&[]);
        eprintln!(
            "TCP HTTP bridge response: {}",
            String::from_utf8_lossy(status).trim()
        );
    }
    remote_reader.set_read_timeout(None)?;
    let downstream = std::io::copy(&mut remote_reader, &mut local_writer);
    let _ = local_writer.shutdown(Shutdown::Both);
    let _ = upstream.join();
    downstream?;
    Ok(())
}
