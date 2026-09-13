// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Optional directory leases and LAN discovery. Workers own all sockets.
use crate::{net::Transport, protocol::PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// Enable only when public connectivity is supported in the product.
pub const PUBLIC_LOBBIES_ENABLED: bool = false;
/// Directory the client asks for public lobbies, as HOST:PORT.
pub const DEFAULT_HOST: &str = "127.0.0.1:7791";
/// UDP port that LAN discovery broadcasts on and an advertisement binds.
pub const LAN_PORT: u16 = 7792;
/// Discovery datagram a client broadcasts and an advertisement answers.
const DISCOVER: &[u8] = b"RM-SIMULATOR-LOBBIES/1";
/// Connect, read and write timeout for one directory exchange.
const TIMEOUT: Duration = Duration::from_secs(3);

/// One advertised match, as the directory and LAN replies carry it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// Display name the host chose, 1 to 64 printable characters.
    pub name: String,
    /// Host address as HOST:PORT. LAN discovery replaces the host part with the
    /// reply's source address and keeps the advertised port.
    pub address: String,
    /// Transport name, `gns` or `tcp`.
    pub transport: String,
    /// Protocol version the host speaks.
    pub protocol: u32,
    /// Whether the host requires a password.
    pub locked: bool,
    /// Whether this entry came from a LAN reply rather than the directory.
    pub lan: bool,
}
impl Listing {
    /// Map the transport name to a [`Transport`]; an unknown name is rejected.
    pub fn transport(&self) -> Result<Transport, String> {
        match self.transport.as_str() {
            "gns" => Ok(Transport::Gns),
            "tcp" => Ok(Transport::Tcp),
            _ => Err("Unsupported lobby transport".into()),
        }
    }
    /// Whether this crate can join the listing: matching protocol and a known
    /// transport.
    pub fn compatible(&self) -> bool {
        self.protocol == PROTOCOL_VERSION && self.transport().is_ok()
    }
}

/// Bounded HTTP/1.0 exchange with the directory; run outside frame updates.
fn request(
    host: &str,
    path: &str,
    body: Option<&serde_json::Value>,
) -> io::Result<serde_json::Value> {
    let address = host
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::other("No lobby host address"))?;
    let mut stream = TcpStream::connect_timeout(&address, TIMEOUT)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let data = body
        .map(serde_json::to_vec)
        .transpose()?
        .unwrap_or_default();
    write!(
        stream,
        "{} {path} HTTP/1.0\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        if body.is_some() { "POST" } else { "GET" },
        data.len()
    )?;
    stream.write_all(&data)?;
    let mut bytes = Vec::new();
    stream.take(262_145).read_to_end(&mut bytes)?;
    if bytes.len() > 262_144 {
        return Err(io::Error::other("Directory response too large"));
    }
    let split = bytes
        .windows(4)
        .position(|s| s == b"\r\n\r\n")
        .ok_or_else(|| io::Error::other("Invalid directory response"))?;
    let headers = String::from_utf8_lossy(&bytes[..split]);
    if !matches!(headers.split_whitespace().nth(1), Some("200" | "201")) {
        return Err(io::Error::other(format!(
            "Directory: {}",
            String::from_utf8_lossy(&bytes[split + 4..])
        )));
    }
    serde_json::from_slice(&bytes[split + 4..]).map_err(io::Error::other)
}

/// Fetch the directory's lobby list and drop malformed entries.
pub fn public_lobbies(host: &str) -> io::Result<Vec<Listing>> {
    let entries: Vec<Listing> = serde_json::from_value(request(host, "/v1/lobbies", None)?)?;
    Ok(entries.into_iter().filter(valid_listing).collect())
}
/// Whether an entry is worth showing: a short printable name, a parseable
/// address and a supported transport.
fn valid_listing(entry: &Listing) -> bool {
    !entry.name.trim().is_empty()
        && entry.name.len() <= 256
        && !entry.name.chars().any(char::is_control)
        && entry.address.parse::<SocketAddr>().is_ok()
        && entry.transport().is_ok()
}

/// Query the LAN for advertisements and collect the replies for 700 ms. Each
/// reply's address is rewritten to the peer's source IP and its advertised
/// port, because a host behind the same NAT cannot know the address peers see.
pub fn lan_lobbies() -> io::Result<Vec<Listing>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_broadcast(true)?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    // VPN default routes can reject the limited broadcast address on macOS.
    // Send to each IPv4 interface's subnet broadcast instead. Failure on one
    // disconnected adapter must not hide hosts on the remaining interfaces.
    for interface in if_addrs::get_if_addrs()? {
        if let if_addrs::IfAddr::V4(address) = interface.addr
            && let Some(broadcast) = address.broadcast
            && !address.ip.is_loopback()
        {
            let _ = socket.send_to(DISCOVER, (broadcast, LAN_PORT));
        }
    }
    // Include a host on this computer even when the OS does not loop broadcasts back.
    socket.send_to(DISCOVER, ("127.0.0.1", LAN_PORT))?;
    let deadline = Instant::now() + Duration::from_millis(700);
    let mut entries = Vec::new();
    let mut buffer = [0; 2048];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buffer) {
            Ok((len, peer)) => {
                if let Ok(mut entry) = serde_json::from_slice::<Listing>(&buffer[..len])
                    && valid_listing(&entry)
                {
                    let port = entry
                        .address
                        .parse::<SocketAddr>()
                        .expect("validated")
                        .port();
                    entry.address = SocketAddr::new(peer.ip(), port).to_string();
                    entry.lan = true;
                    if !entries.contains(&entry) && entries.len() < 128 {
                        entries.push(entry);
                    }
                }
            }
            // A probe can reach a closed UDP port. Windows reports the ICMP
            // response as ConnectionReset; keep collecting other hosts' replies.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionRefused
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(entries)
}

/// Owned by the match. Drop signals withdrawal without blocking the render thread.
/// A crashed host's public lease expires after 45 seconds.
/// Dropping it closes the stop channel, so the worker withdraws a public
/// listing and releases [`LAN_PORT`].
pub struct Advertisement {
    stop: Option<mpsc::Sender<()>>,
}
impl Advertisement {
    /// Advertise a match and answer discovery until the returned value drops.
    /// `address` supplies the advertised port, and the whole listing address
    /// when `advertised` is empty. `advertised` overrides the address a public
    /// directory publishes, which matters behind NAT. `host` is the directory
    /// as HOST:PORT. A LAN advertisement binds [`LAN_PORT`], so only one runs
    /// per computer; a public listing registers with the directory first and
    /// fails if that request fails. A name that is empty, longer than 64
    /// characters or contains a control character is refused.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        host: &str,
        name: &str,
        address: SocketAddr,
        transport: Transport,
        locked: bool,
        public: bool,
        advertised: &str,
    ) -> io::Result<Self> {
        if public && !PUBLIC_LOBBIES_ENABLED {
            return Err(io::Error::other("Public lobbies are currently disabled"));
        }
        if name.trim().is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control)
        {
            return Err(io::Error::other(
                "Lobby name must be 1 to 64 printable characters",
            ));
        }
        let mut listing = Listing {
            name: name.trim().into(),
            address: address.to_string(),
            transport: match transport {
                Transport::Gns => "gns",
                Transport::Tcp => "tcp",
            }
            .into(),
            protocol: PROTOCOL_VERSION,
            locked,
            lan: true,
        };
        let socket = UdpSocket::bind(("0.0.0.0", LAN_PORT)).map_err(|e| io::Error::new(e.kind(), format!("Cannot open LAN discovery port {LAN_PORT}: {e}. Close any other advertised lobby on this computer.")))?;
        socket.set_nonblocking(true)?;
        let lan = serde_json::to_vec(&listing)?;
        let advertised = if !public || advertised.trim().is_empty() {
            None
        } else {
            Some(advertised.parse::<SocketAddr>().map_err(io::Error::other)?)
        };
        if let Some(address) = advertised {
            listing.address = address.to_string();
        }
        let registration = serde_json::json!({"name": listing.name, "port": advertised.unwrap_or(address).port(), "ip": advertised.map(|a| a.ip().to_string()), "transport": listing.transport, "protocol": PROTOCOL_VERSION, "locked": locked});
        let host = host.to_owned();
        let register = || -> io::Result<String> {
            request(&host, "/v1/lobbies", Some(&registration))?["token"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| io::Error::other("Directory omitted lease token"))
        };
        // Initial registration must succeed before claiming that a public lobby exists.
        let mut token = if public { Some(register()?) } else { None };
        let (stop, stopping) = mpsc::channel();
        thread::Builder::new()
            .name("rm-lobby-advertise".into())
            .spawn(move || {
                let mut heartbeat = Instant::now() + Duration::from_secs(10);
                while let Err(mpsc::RecvTimeoutError::Timeout) =
                    stopping.recv_timeout(Duration::from_millis(50))
                {
                    let mut buffer = [0; 128];
                    // Bound work per iteration so a broadcast flood cannot starve shutdown.
                    for _ in 0..16 {
                        let Ok((len, peer)) = socket.recv_from(&mut buffer) else {
                            break;
                        };
                        if &buffer[..len] == DISCOVER {
                            let _ = socket.send_to(&lan, peer);
                        }
                    }
                    if public && Instant::now() >= heartbeat {
                        let result = token.as_ref().map(|token| {
                            request(
                                &host,
                                "/v1/heartbeat",
                                Some(&serde_json::json!({"token": token})),
                            )
                        });
                        if !matches!(result, Some(Ok(_))) {
                            token = request(&host, "/v1/lobbies", Some(&registration))
                                .ok()
                                .and_then(|v| v["token"].as_str().map(str::to_owned));
                        }
                        heartbeat = Instant::now() + Duration::from_secs(10);
                    }
                }
                if let Some(token) = token {
                    let _ = request(
                        &host,
                        "/v1/remove",
                        Some(&serde_json::json!({"token": token})),
                    );
                }
            })?;
        Ok(Self { stop: Some(stop) })
    }
}
impl Drop for Advertisement {
    /// Close the stop channel, which ends the advertising worker.
    fn drop(&mut self) {
        self.stop.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        net::{Client, Server},
        protocol::Role,
        simulation::Simulation,
    };
    use rm_simulator_world::{Field, FieldConfig, Team};

    #[test]
    fn password_admission_covers_both_transports_and_owner_bypass() {
        let _serial = crate::net::NATIVE_TEST
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for transport in [Transport::Tcp, Transport::Gns] {
            let simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), true)
                .with_password("secret".into());
            let server = match transport {
                Transport::Tcp => Server::bind("127.0.0.1:0", simulation),
                Transport::Gns => Server::bind_udp("127.0.0.1:0", simulation),
            }
            .unwrap();
            let join = |password: &str| match transport {
                Transport::Tcp => Client::connect_with_password(
                    server.local_addr(),
                    "guest",
                    None,
                    Role::Spectator,
                    password,
                ),
                Transport::Gns => Client::connect_udp_with_password(
                    server.local_addr(),
                    "guest",
                    None,
                    Role::Spectator,
                    password,
                ),
            };
            assert!(join("").is_err());
            assert!(join("wrong").is_err());
            assert_eq!(server.peer_count(), 0);
            let _guest = join("secret").unwrap();
            assert_eq!(server.peer_count(), 1);
            let _owner = server
                .connect_owner("owner", Team::Red, Role::Spectator, [0.; 3], 0.)
                .unwrap();
            assert_eq!(server.peer_count(), 2);
        }
    }

    #[test]
    fn lan_discovery_never_contacts_the_directory_or_leaks_the_password() {
        let advert = Advertisement::start(
            "invalid directory",
            "Local test",
            "0.0.0.0:17700".parse().unwrap(),
            Transport::Gns,
            true,
            false,
            "",
        )
        .unwrap();
        let entries = lan_lobbies().unwrap();
        assert!(entries.iter().any(|entry| entry.name == "Local test"
            && entry.locked
            && entry.lan
            && entry.address.ends_with(":17700")
            && entry.compatible()));
        drop(advert);
        // Allow the owned worker to observe shutdown; bounded by its 50 ms poll.
        let deadline = Instant::now() + Duration::from_secs(2);
        while UdpSocket::bind(("0.0.0.0", LAN_PORT)).is_err() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(lan_lobbies().unwrap().is_empty());
    }
}
