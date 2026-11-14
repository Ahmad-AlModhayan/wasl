//! Asking the home gateway for a port (docs/DEVICE_SYNC.md §7): PCP (RFC 6887) MAP, falling back
//! to NAT-PMP (RFC 6886), for the QUIC endpoint's UDP port.
//!
//! A mapping has a lifetime: renew it at [`Mapping::renew_at`] (half the lifetime, as both RFCs
//! advise) and delete it ([`unmap`]) when the device stops or reach changes to `lan`. Requests are
//! retransmitted with a doubling timeout, bounded by [`PortmapConfig`]; answers are accepted only
//! from the gateway's address and port, and only when they match the request (opcode, ports, and
//! for PCP the 96-bit nonce). Gateways are found by [`gateway_guesses`]; the caller may know better.
//!
//! UPnP IGD is not implemented: it needs SSDP discovery, an HTTP client and XML/SOAP, which is
//! more surface than it is worth next to PCP/NAT-PMP and hole punching.
//! TODO(M4.5): UPnP IGD `AddPortMapping` for routers that speak neither PCP nor NAT-PMP.
//!
//! What an attacker on the local network can do: answer for the gateway (neither protocol is
//! authenticated), so this device publishes a wrong mapped address in its signed record. Peers
//! then try a QUIC handshake there and the pinned-key check fails: a lost path, never a connection
//! to someone else. Or it can ask the gateway for mappings itself, which it could anyway.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;

/// The port PCP and NAT-PMP servers listen on.
pub const SERVER_PORT: u16 = 5351;
/// The PCP version spoken.
pub const PCP_VERSION: u8 = 2;
/// Length of a PCP MAP request and response.
pub const PCP_MAP_LEN: usize = 60;
/// The largest PCP message (RFC 6887 §7); anything longer is ignored.
pub const MAX_MESSAGE_BYTES: usize = 1100;

const PCP_OP_MAP: u8 = 1;
const PCP_RESPONSE: u8 = 0x80;
const PROTO_UDP: u8 = 17;
const NATPMP_OP_EXTERNAL: u8 = 0;
const NATPMP_OP_MAP_UDP: u8 = 1;
const NATPMP_RESPONSE: u8 = 0x80;

/// Why a port mapping failed.
#[derive(Debug, thiserror::Error)]
pub enum PortmapError {
    #[error("no answer from the gateway")]
    Timeout,
    #[error("the gateway refused the mapping (result {0})")]
    Refused(u16),
    #[error("the gateway does not speak PCP")]
    PcpUnsupported,
    #[error("malformed answer: {0}")]
    Malformed(&'static str),
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
}

/// Which protocol made a mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Pcp,
    NatPmp,
}

/// A UDP port the gateway forwards to this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub gateway: SocketAddr,
    pub protocol: Protocol,
    pub internal_port: u16,
    /// The public address and port peers can send to.
    pub external: SocketAddr,
    pub lifetime: Duration,
    pub obtained: Instant,
    nonce: [u8; 12],
}

impl Mapping {
    /// When to renew: half the lifetime.
    pub fn renew_at(&self) -> Instant {
        self.obtained + self.lifetime / 2
    }

    /// When the gateway drops it unless renewed.
    pub fn expires_at(&self) -> Instant {
        self.obtained + self.lifetime
    }
}

/// How hard requests are tried, and the lifetime asked for.
#[derive(Debug, Clone)]
pub struct PortmapConfig {
    /// The first retransmission timeout, doubled after each send (both RFCs start at 250 ms).
    pub rto: Duration,
    /// Requests sent at most per protocol.
    pub sends: u32,
    /// The lifetime asked for (the gateway may give less).
    pub lifetime: Duration,
    /// Try PCP before NAT-PMP.
    pub pcp: bool,
}

impl Default for PortmapConfig {
    fn default() -> Self {
        Self {
            rto: Duration::from_millis(250),
            sends: 4,
            lifetime: Duration::from_secs(7200),
            pcp: true,
        }
    }
}

/// A PCP MAP request for UDP `internal` port (RFC 6887 §11.1). `client` is this device's address
/// as the gateway sees it; `suggested` the external port wished for (0: any).
pub fn pcp_map_request(
    client: IpAddr,
    nonce: &[u8; 12],
    internal: u16,
    suggested: u16,
    lifetime_secs: u32,
) -> [u8; PCP_MAP_LEN] {
    let mut out = [0u8; PCP_MAP_LEN];
    out[0] = PCP_VERSION;
    out[1] = PCP_OP_MAP;
    out[4..8].copy_from_slice(&lifetime_secs.to_be_bytes());
    out[8..24].copy_from_slice(&mapped_v6(client).octets());
    out[24..36].copy_from_slice(nonce);
    out[36] = PROTO_UDP;
    out[40..42].copy_from_slice(&internal.to_be_bytes());
    out[42..44].copy_from_slice(&suggested.to_be_bytes());
    // No preferred external address: the all-zero address of the client's family.
    let any = match client {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    out[44..60].copy_from_slice(&mapped_v6(any).octets());
    out
}

fn mapped_v6(ip: IpAddr) -> Ipv6Addr {
    match ip {
        IpAddr::V4(v4) => v4.to_ipv6_mapped(),
        IpAddr::V6(v6) => v6,
    }
}

/// A decoded PCP MAP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcpMapResponse {
    pub result: u8,
    pub lifetime_secs: u32,
    pub epoch: u32,
    pub nonce: [u8; 12],
    pub protocol: u8,
    pub internal: u16,
    pub external_port: u16,
    pub external_ip: IpAddr,
}

/// Decodes a PCP MAP response. A version-0 answer means a NAT-PMP-only gateway
/// ([`PortmapError::PcpUnsupported`]).
pub fn decode_pcp(bytes: &[u8]) -> Result<PcpMapResponse, PortmapError> {
    if bytes.len() < 4 {
        return Err(PortmapError::Malformed("short PCP answer"));
    }
    if bytes[0] == 0 {
        return Err(PortmapError::PcpUnsupported);
    }
    if bytes[0] != PCP_VERSION {
        return Err(PortmapError::Malformed("unknown PCP version"));
    }
    if bytes[1] != PCP_RESPONSE | PCP_OP_MAP {
        return Err(PortmapError::Malformed("not a MAP response"));
    }
    if bytes.len() < PCP_MAP_LEN || bytes.len() > MAX_MESSAGE_BYTES || !bytes.len().is_multiple_of(4) {
        return Err(PortmapError::Malformed("bad PCP answer length"));
    }
    let u16_at = |i: usize| u16::from_be_bytes([bytes[i], bytes[i + 1]]);
    let u32_at = |i: usize| u32::from_be_bytes(bytes[i..i + 4].try_into().expect("4 bytes"));
    let ip = Ipv6Addr::from(<[u8; 16]>::try_from(&bytes[44..60]).expect("16 bytes"));
    Ok(PcpMapResponse {
        result: bytes[3],
        lifetime_secs: u32_at(4),
        epoch: u32_at(8),
        nonce: bytes[24..36].try_into().expect("12 bytes"),
        protocol: bytes[36],
        internal: u16_at(40),
        external_port: u16_at(42),
        external_ip: IpAddr::V6(ip).to_canonical(),
    })
}

/// The NAT-PMP request for the gateway's external address.
pub fn natpmp_external_request() -> [u8; 2] {
    [0, NATPMP_OP_EXTERNAL]
}

/// A NAT-PMP UDP mapping request (RFC 6886 §3.3). Lifetime 0 with suggested 0 deletes.
pub fn natpmp_map_request(internal: u16, suggested: u16, lifetime_secs: u32) -> [u8; 12] {
    let mut out = [0u8; 12];
    out[1] = NATPMP_OP_MAP_UDP;
    out[4..6].copy_from_slice(&internal.to_be_bytes());
    out[6..8].copy_from_slice(&suggested.to_be_bytes());
    out[8..12].copy_from_slice(&lifetime_secs.to_be_bytes());
    out
}

/// A decoded NAT-PMP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NatPmpResponse {
    External {
        result: u16,
        epoch: u32,
        ip: Ipv4Addr,
    },
    Map {
        result: u16,
        epoch: u32,
        internal: u16,
        external: u16,
        lifetime_secs: u32,
    },
}

/// Decodes a NAT-PMP response; lengths are checked before every read.
pub fn decode_natpmp(bytes: &[u8]) -> Result<NatPmpResponse, PortmapError> {
    if bytes.len() < 8 || bytes[0] != 0 {
        return Err(PortmapError::Malformed("not a NAT-PMP answer"));
    }
    let result = u16::from_be_bytes([bytes[2], bytes[3]]);
    let epoch = u32::from_be_bytes(bytes[4..8].try_into().expect("4 bytes"));
    match bytes[1] {
        op if op == NATPMP_RESPONSE | NATPMP_OP_EXTERNAL => {
            if result != 0 {
                return Ok(NatPmpResponse::External {
                    result,
                    epoch,
                    ip: Ipv4Addr::UNSPECIFIED,
                });
            }
            if bytes.len() < 12 {
                return Err(PortmapError::Malformed("short external address answer"));
            }
            let ip = Ipv4Addr::from(<[u8; 4]>::try_from(&bytes[8..12]).expect("4 bytes"));
            Ok(NatPmpResponse::External { result, epoch, ip })
        }
        op if op == NATPMP_RESPONSE | NATPMP_OP_MAP_UDP => {
            if result != 0 {
                return Ok(NatPmpResponse::Map {
                    result,
                    epoch,
                    internal: 0,
                    external: 0,
                    lifetime_secs: 0,
                });
            }
            if bytes.len() < 16 {
                return Err(PortmapError::Malformed("short mapping answer"));
            }
            Ok(NatPmpResponse::Map {
                result,
                epoch,
                internal: u16::from_be_bytes([bytes[8], bytes[9]]),
                external: u16::from_be_bytes([bytes[10], bytes[11]]),
                lifetime_secs: u32::from_be_bytes(bytes[12..16].try_into().expect("4 bytes")),
            })
        }
        _ => Err(PortmapError::Malformed("unexpected NAT-PMP opcode")),
    }
}

/// Asks `gateway` (normally `<router>:`[`SERVER_PORT`]) to forward a public UDP port to
/// `internal_port` on this device: PCP first (if enabled), then NAT-PMP.
pub async fn map_udp(
    gateway: SocketAddr,
    internal_port: u16,
    cfg: &PortmapConfig,
) -> Result<Mapping, PortmapError> {
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).expect("the OS random source is available");
    request(gateway, internal_port, 0, cfg.lifetime, nonce, None, cfg).await
}

/// Renews `mapping`, asking for the same external port.
pub async fn renew(mapping: &Mapping, cfg: &PortmapConfig) -> Result<Mapping, PortmapError> {
    request(
        mapping.gateway,
        mapping.internal_port,
        mapping.external.port(),
        cfg.lifetime,
        mapping.nonce,
        Some(mapping.protocol),
        cfg,
    )
    .await
}

/// Deletes `mapping` at the gateway.
pub async fn unmap(mapping: &Mapping, cfg: &PortmapConfig) -> Result<(), PortmapError> {
    let socket = connect(mapping.gateway).await?;
    match mapping.protocol {
        Protocol::Pcp => {
            let client = socket.local_addr()?.ip();
            let req = pcp_map_request(client, &mapping.nonce, mapping.internal_port, 0, 0);
            pcp_exchange(&socket, &req, &mapping.nonce, mapping.internal_port, cfg).await?;
        }
        Protocol::NatPmp => {
            let req = natpmp_map_request(mapping.internal_port, 0, 0);
            natpmp_map_exchange(&socket, &req, mapping.internal_port, cfg).await?;
        }
    }
    Ok(())
}

async fn request(
    gateway: SocketAddr,
    internal: u16,
    suggested: u16,
    lifetime: Duration,
    nonce: [u8; 12],
    only: Option<Protocol>,
    cfg: &PortmapConfig,
) -> Result<Mapping, PortmapError> {
    let socket = connect(gateway).await?;
    let lifetime_secs = u32::try_from(lifetime.as_secs()).unwrap_or(u32::MAX).max(1);
    let try_pcp = match only {
        Some(p) => p == Protocol::Pcp,
        None => cfg.pcp,
    };
    if try_pcp {
        let client = socket.local_addr()?.ip();
        let req = pcp_map_request(client, &nonce, internal, suggested, lifetime_secs);
        match pcp_exchange(&socket, &req, &nonce, internal, cfg).await {
            Ok(resp) => {
                return Ok(Mapping {
                    gateway,
                    protocol: Protocol::Pcp,
                    internal_port: internal,
                    external: SocketAddr::new(resp.external_ip, resp.external_port),
                    lifetime: Duration::from_secs(resp.lifetime_secs.into()),
                    obtained: Instant::now(),
                    nonce,
                });
            }
            Err(e @ PortmapError::Refused(_)) => return Err(e),
            Err(e) if only == Some(Protocol::Pcp) => return Err(e),
            // No PCP (a version-0 answer, or silence): NAT-PMP.
            Err(_) => {}
        }
    }
    if !gateway.is_ipv4() {
        return Err(PortmapError::Malformed("NAT-PMP needs an IPv4 gateway"));
    }
    let ip = exchange(
        &socket,
        &natpmp_external_request(),
        cfg,
        |bytes| match decode_natpmp(bytes) {
            Ok(NatPmpResponse::External { result: 0, ip, .. }) => Some(Ok(ip)),
            Ok(NatPmpResponse::External { result, .. }) => Some(Err(PortmapError::Refused(result))),
            _ => None,
        },
    )
    .await?;
    let req = natpmp_map_request(internal, suggested, lifetime_secs);
    let (external, lifetime_secs) = natpmp_map_exchange(&socket, &req, internal, cfg).await?;
    Ok(Mapping {
        gateway,
        protocol: Protocol::NatPmp,
        internal_port: internal,
        external: SocketAddr::new(IpAddr::V4(ip), external),
        lifetime: Duration::from_secs(lifetime_secs.into()),
        obtained: Instant::now(),
        nonce,
    })
}

async fn connect(gateway: SocketAddr) -> Result<UdpSocket, PortmapError> {
    let any: SocketAddr = if gateway.is_ipv4() {
        (Ipv4Addr::UNSPECIFIED, 0).into()
    } else {
        (Ipv6Addr::UNSPECIFIED, 0).into()
    };
    let socket = UdpSocket::bind(any).await?;
    // Connected: the OS drops datagrams from any other address, and local_addr() is the address
    // the gateway sees.
    socket.connect(gateway).await?;
    Ok(socket)
}

async fn pcp_exchange(
    socket: &UdpSocket,
    req: &[u8],
    nonce: &[u8; 12],
    internal: u16,
    cfg: &PortmapConfig,
) -> Result<PcpMapResponse, PortmapError> {
    exchange(socket, req, cfg, |bytes| match decode_pcp(bytes) {
        Err(PortmapError::PcpUnsupported) => Some(Err(PortmapError::PcpUnsupported)),
        Ok(r) if &r.nonce == nonce && r.protocol == PROTO_UDP && r.internal == internal => {
            Some(match r.result {
                0 => Ok(r),
                code => Err(PortmapError::Refused(code.into())),
            })
        }
        _ => None,
    })
    .await
}

async fn natpmp_map_exchange(
    socket: &UdpSocket,
    req: &[u8],
    internal: u16,
    cfg: &PortmapConfig,
) -> Result<(u16, u32), PortmapError> {
    exchange(socket, req, cfg, |bytes| match decode_natpmp(bytes) {
        Ok(NatPmpResponse::Map {
            result: 0,
            internal: i,
            external,
            lifetime_secs,
            ..
        }) if i == internal => Some(Ok((external, lifetime_secs))),
        Ok(NatPmpResponse::Map { result, .. }) if result != 0 => Some(Err(PortmapError::Refused(result))),
        _ => None,
    })
    .await
}

/// Sends `req` with retransmissions until `accept` takes an answer.
async fn exchange<T>(
    socket: &UdpSocket,
    req: &[u8],
    cfg: &PortmapConfig,
    mut accept: impl FnMut(&[u8]) -> Option<Result<T, PortmapError>>,
) -> Result<T, PortmapError> {
    let mut buf = [0u8; MAX_MESSAGE_BYTES + 1];
    let mut rto = cfg.rto;
    for _ in 0..cfg.sends.max(1) {
        socket.send(req).await?;
        let until = tokio::time::Instant::now() + rto;
        rto = rto.saturating_mul(2);
        loop {
            match tokio::time::timeout_at(until, socket.recv(&mut buf)).await {
                Err(_) => break,
                Ok(Ok(n)) if n <= MAX_MESSAGE_BYTES => {
                    if let Some(answer) = accept(&buf[..n]) {
                        return answer;
                    }
                }
                Ok(Ok(_)) => {}
                // An ICMP "port unreachable" (no server there) on a connected socket.
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                    return Err(PortmapError::Timeout);
                }
                Ok(Err(e)) => return Err(e.into()),
            }
        }
    }
    Err(PortmapError::Timeout)
}

/// Likely gateways, best first: the IPv4 default routes (Linux and Android, where readable), then
/// the first address of each private IPv4 network this device is on.
pub fn gateway_guesses() -> Vec<SocketAddr> {
    let mut out: Vec<Ipv4Addr> = Vec::new();
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if let Ok(text) = std::fs::read_to_string("/proc/net/route") {
        out.extend(parse_proc_route(&text));
    }
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if let if_addrs::IfAddr::V4(v4) = iface.addr
                && v4.ip.is_private()
                && v4.prefixlen < 31
            {
                let network = u32::from(v4.ip) & u32::from(v4.netmask);
                let first = Ipv4Addr::from(network + 1);
                if first != v4.ip && !out.contains(&first) {
                    out.push(first);
                }
            }
        }
    }
    out.into_iter()
        .map(|ip| SocketAddr::from((ip, SERVER_PORT)))
        .collect()
}

/// The gateways of the IPv4 default routes in `/proc/net/route` (hex, host byte order).
pub fn parse_proc_route(text: &str) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for line in text.lines().skip(1).take(256) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 || fields[1] != "00000000" {
            continue;
        }
        if let Ok(raw) = u32::from_str_radix(fields[2], 16) {
            let ip = Ipv4Addr::from(raw.to_le_bytes());
            if !ip.is_unspecified() && !out.contains(&ip) {
                out.push(ip);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn pcp_request_layout() {
        let nonce = [7u8; 12];
        let req = pcp_map_request("192.168.1.20".parse().unwrap(), &nonce, 4433, 0, 7200);
        assert_eq!(req[0], 2);
        assert_eq!(req[1], 1);
        assert_eq!(&req[4..8], &7200u32.to_be_bytes());
        assert_eq!(
            &req[8..24],
            &"::ffff:192.168.1.20".parse::<Ipv6Addr>().unwrap().octets()
        );
        assert_eq!(&req[24..36], &nonce);
        assert_eq!(req[36], 17);
        assert_eq!(&req[40..42], &4433u16.to_be_bytes());
        assert_eq!(
            &req[44..60],
            &"::ffff:0.0.0.0".parse::<Ipv6Addr>().unwrap().octets()
        );
    }

    fn pcp_response(req: &[u8], result: u8, external: SocketAddr, lifetime: u32) -> Vec<u8> {
        let mut out = req.to_vec();
        out[1] = 0x81;
        out[2] = 0;
        out[3] = result;
        out[4..8].copy_from_slice(&lifetime.to_be_bytes());
        out[8..12].copy_from_slice(&99u32.to_be_bytes());
        out[12..24].fill(0);
        out[42..44].copy_from_slice(&external.port().to_be_bytes());
        out[44..60].copy_from_slice(&mapped_v6(external.ip()).octets());
        out
    }

    #[test]
    fn pcp_response_decodes() {
        let req = pcp_map_request("10.0.0.2".parse().unwrap(), &[3; 12], 4433, 0, 60);
        let resp = decode_pcp(&pcp_response(&req, 0, "198.51.100.7:40000".parse().unwrap(), 60)).unwrap();
        assert_eq!(resp.external_ip, "198.51.100.7".parse::<IpAddr>().unwrap());
        assert_eq!(resp.external_port, 40000);
        assert_eq!(resp.nonce, [3; 12]);
        assert_eq!(resp.internal, 4433);
        assert_eq!(resp.lifetime_secs, 60);
        assert_eq!(resp.epoch, 99);
        assert!(matches!(
            decode_pcp(&[0, 0x81, 0, 1, 0, 0, 0, 0]),
            Err(PortmapError::PcpUnsupported)
        ));
        assert!(decode_pcp(&req).is_err(), "a request is not a response");
        assert!(decode_pcp(&[2, 0x81, 0]).is_err());
        assert!(decode_pcp(&[2, 0x81, 0, 0, 0]).is_err());
    }

    #[test]
    fn natpmp_codec() {
        assert_eq!(natpmp_external_request(), [0, 0]);
        assert_eq!(
            natpmp_map_request(4433, 0, 3600),
            [0, 1, 0, 0, 0x11, 0x51, 0, 0, 0, 0, 0x0e, 0x10]
        );
        assert_eq!(
            decode_natpmp(&[0, 128, 0, 0, 0, 0, 0, 5, 203, 0, 113, 4]).unwrap(),
            NatPmpResponse::External {
                result: 0,
                epoch: 5,
                ip: "203.0.113.4".parse().unwrap()
            }
        );
        assert_eq!(
            decode_natpmp(&[0, 129, 0, 0, 0, 0, 0, 5, 0x11, 0x51, 0x9c, 0x40, 0, 0, 0x0e, 0x10]).unwrap(),
            NatPmpResponse::Map {
                result: 0,
                epoch: 5,
                internal: 4433,
                external: 40000,
                lifetime_secs: 3600
            }
        );
        assert!(decode_natpmp(&[0, 128, 0, 0, 0, 0, 0, 5]).is_err(), "short");
        assert!(decode_natpmp(&[0, 129, 0, 0, 0, 0, 0, 5, 1]).is_err(), "short");
        assert!(decode_natpmp(&[2, 129, 0, 0, 0, 0, 0, 5]).is_err(), "version");
        assert!(decode_natpmp(&[0, 1, 0, 0, 0, 0, 0, 5]).is_err(), "a request");
        assert!(matches!(
            decode_natpmp(&[0, 129, 0, 3, 0, 0, 0, 5]).unwrap(),
            NatPmpResponse::Map { result: 3, .. }
        ));
    }

    #[test]
    fn proc_route_default_gateway() {
        let text = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\n\
                    eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\n\
                    eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\n";
        assert_eq!(
            parse_proc_route(text),
            vec!["192.168.1.1".parse::<Ipv4Addr>().unwrap()]
        );
        assert!(parse_proc_route("garbage\nx y\n").is_empty());
    }

    /// What a fake gateway saw.
    #[derive(Default)]
    struct Seen {
        pcp: Vec<Vec<u8>>,
        natpmp: Vec<Vec<u8>>,
    }

    /// A gateway on loopback: PCP if `pcp`, else NAT-PMP only. Maps every port to 40000 at
    /// 198.51.100.7, unless `refuse`.
    async fn fake_gateway(pcp: bool, refuse: Option<u8>) -> (SocketAddr, Arc<Mutex<Seen>>) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let log = seen.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1200];
            loop {
                let Ok((n, from)) = socket.recv_from(&mut buf).await else {
                    return;
                };
                let req = buf[..n].to_vec();
                let reply = if req[0] == PCP_VERSION {
                    log.lock().unwrap().pcp.push(req.clone());
                    if !pcp {
                        vec![0, 0x81, 0, 1, 0, 0, 0, 1]
                    } else {
                        let lifetime = u32::from_be_bytes(req[4..8].try_into().unwrap()).min(600);
                        pcp_response(
                            &req,
                            refuse.unwrap_or(0),
                            "198.51.100.7:40000".parse().unwrap(),
                            lifetime,
                        )
                    }
                } else {
                    log.lock().unwrap().natpmp.push(req.clone());
                    match req[1] {
                        0 => vec![0, 128, 0, 0, 0, 0, 0, 1, 198, 51, 100, 7],
                        1 => {
                            let mut r = vec![0, 129, 0, refuse.unwrap_or(0), 0, 0, 0, 1];
                            r.extend_from_slice(&req[4..6]);
                            r.extend_from_slice(&40000u16.to_be_bytes());
                            let lifetime = u32::from_be_bytes(req[8..12].try_into().unwrap()).min(600);
                            r.extend_from_slice(&lifetime.to_be_bytes());
                            r
                        }
                        _ => continue,
                    }
                };
                let _ = socket.send_to(&reply, from).await;
            }
        });
        (addr, seen)
    }

    fn quick() -> PortmapConfig {
        PortmapConfig {
            rto: Duration::from_millis(50),
            sends: 3,
            ..PortmapConfig::default()
        }
    }

    #[tokio::test]
    async fn pcp_map_renew_unmap() {
        let (gateway, seen) = fake_gateway(true, None).await;
        let m = map_udp(gateway, 4433, &quick()).await.unwrap();
        assert_eq!(m.protocol, Protocol::Pcp);
        assert_eq!(m.external, "198.51.100.7:40000".parse().unwrap());
        assert_eq!(m.lifetime, Duration::from_secs(600));
        assert!(m.renew_at() < m.expires_at());
        let renewed = renew(&m, &quick()).await.unwrap();
        assert_eq!(renewed.external, m.external);
        unmap(&m, &quick()).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.pcp.len(), 3);
        assert!(seen.natpmp.is_empty());
        // The renewal reuses the nonce and asks for the same port; the deletion has lifetime 0.
        assert_eq!(seen.pcp[1][24..36], seen.pcp[0][24..36]);
        assert_eq!(&seen.pcp[1][42..44], &40000u16.to_be_bytes());
        assert_eq!(&seen.pcp[2][4..8], &[0, 0, 0, 0]);
    }

    #[tokio::test]
    async fn natpmp_only_gateway_falls_back() {
        let (gateway, seen) = fake_gateway(false, None).await;
        let m = map_udp(gateway, 4433, &quick()).await.unwrap();
        assert_eq!(m.protocol, Protocol::NatPmp);
        assert_eq!(m.external, "198.51.100.7:40000".parse().unwrap());
        unmap(&m, &quick()).await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.pcp.len(), 1, "one PCP try, answered with version 0");
        assert_eq!(seen.natpmp.len(), 3, "external address, map, delete");
        assert_eq!(&seen.natpmp[2][8..12], &[0, 0, 0, 0]);
    }

    #[tokio::test]
    async fn a_refusal_is_reported() {
        let (gateway, _) = fake_gateway(true, Some(2)).await;
        assert!(matches!(
            map_udp(gateway, 4433, &quick()).await,
            Err(PortmapError::Refused(2))
        ));
    }

    #[tokio::test]
    async fn silence_gives_up_after_bounded_sends() {
        let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let gateway = silent.local_addr().unwrap();
        let count = tokio::spawn(async move {
            let mut buf = [0u8; 100];
            let mut n = 0;
            while let Ok(Ok(_)) =
                tokio::time::timeout(Duration::from_millis(800), silent.recv_from(&mut buf)).await
            {
                n += 1;
            }
            n
        });
        let cfg = PortmapConfig {
            rto: Duration::from_millis(20),
            sends: 2,
            ..PortmapConfig::default()
        };
        assert!(matches!(
            map_udp(gateway, 1, &cfg).await,
            Err(PortmapError::Timeout)
        ));
        // Two PCP tries, then two for the NAT-PMP external address.
        assert_eq!(count.await.unwrap(), 4);
    }

    #[tokio::test]
    async fn answers_from_elsewhere_or_for_other_requests_are_ignored() {
        // A "gateway" that answers with the wrong nonce: never accepted.
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let gateway = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 100];
            while let Ok((n, from)) = socket.recv_from(&mut buf).await {
                if buf[0] == PCP_VERSION {
                    let mut req = buf[..n].to_vec();
                    req[24] ^= 1;
                    let reply = pcp_response(&req, 0, "6.6.6.6:6".parse().unwrap(), 60);
                    let _ = socket.send_to(&reply, from).await;
                }
            }
        });
        let cfg = PortmapConfig {
            rto: Duration::from_millis(20),
            sends: 2,
            ..PortmapConfig::default()
        };
        assert!(map_udp(gateway, 1, &cfg).await.is_err());
    }
}
