//! The NAT lab (docs/DEVICE_SYNC.md §8 phase 5): two devices behind two different NATs, built
//! from Linux network namespaces, find each other through STUN and a shared rendezvous and connect
//! by hole punching.
//!
//! ```text
//!   wl-a 10.0.1.2 ── 10.0.1.1 wl-nat-a 198.51.100.2 ─┐
//!                                                   br0 198.51.100.1 (wl-wan: STUN server)
//!   wl-b 10.0.2.2 ── 10.0.2.1 wl-nat-b 198.51.100.3 ─┘
//! ```
//!
//! Each NAT masquerades its LAN and, like a home router, drops unsolicited packets from the WAN
//! (so the first packets of a hole punch are lost, as they are in real life). The rendezvous is a
//! directory both devices read and write, standing in for the Drive app-data folder. Each role runs
//! as a child process of this test (`ip netns exec`), re-running this test binary with
//! `WASL_NAT_LAB_ROLE` set.
//!
//! Needs root and `ip`/`iptables`; runs only with `WASL_NAT_LAB=1` (see
//! `.github/workflows/nat-lab.yml`), and is skipped with a note otherwise.

#![cfg(target_os = "linux")]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use wasl_chunk::{ChunkList, Hash};
use wasl_core::Reach;
use wasl_net::holepunch::{self, PunchConfig, Via};
use wasl_net::rendezvous::{self, PeerRecord, Rendezvous, RendezvousError, ReplayGuard};
use wasl_net::stun::{self, StunConfig};
use wasl_net::{
    DeviceId, DeviceKey, Endpoint, IndexPage, PeerError, PeerLink, PeerResult, PeerService, ShareId, Trust,
};

const NAMESPACES: [&str; 5] = ["wl-wan", "wl-nat-a", "wl-nat-b", "wl-a", "wl-b"];
const STUN_ADDR: &str = "198.51.100.1:3478";
const CHILD_TIMEOUT: Duration = Duration::from_secs(120);

fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1).map(|u| u == "0"))
        })
        .unwrap_or(false)
}

fn run(args: &[&str]) {
    let status = Command::new(args[0])
        .args(&args[1..])
        .status()
        .unwrap_or_else(|e| panic!("{args:?}: {e}"));
    assert!(status.success(), "{args:?} failed: {status}");
}

/// The namespaces, removed on drop.
struct Lab;

impl Lab {
    fn build() -> Self {
        Self::clean();
        for ns in NAMESPACES {
            run(&["ip", "netns", "add", ns]);
            run(&["ip", "-n", ns, "link", "set", "lo", "up"]);
        }
        run(&["ip", "-n", "wl-wan", "link", "add", "br0", "type", "bridge"]);
        run(&[
            "ip",
            "-n",
            "wl-wan",
            "addr",
            "add",
            "198.51.100.1/24",
            "dev",
            "br0",
        ]);
        run(&["ip", "-n", "wl-wan", "link", "set", "br0", "up"]);
        for (side, wan_ip, net) in [
            ("a", "198.51.100.2/24", "10.0.1"),
            ("b", "198.51.100.3/24", "10.0.2"),
        ] {
            let nat = format!("wl-nat-{side}");
            let host = format!("wl-{side}");
            let port = format!("p{side}");
            run(&[
                "ip", "-n", "wl-wan", "link", "add", &port, "type", "veth", "peer", "name", "wan0", "netns",
                &nat,
            ]);
            run(&["ip", "-n", "wl-wan", "link", "set", &port, "master", "br0"]);
            run(&["ip", "-n", "wl-wan", "link", "set", &port, "up"]);
            run(&["ip", "-n", &nat, "addr", "add", wan_ip, "dev", "wan0"]);
            run(&["ip", "-n", &nat, "link", "set", "wan0", "up"]);
            run(&[
                "ip", "-n", &nat, "link", "add", "lan0", "type", "veth", "peer", "name", "eth0", "netns",
                &host,
            ]);
            run(&[
                "ip",
                "-n",
                &nat,
                "addr",
                "add",
                &format!("{net}.1/24"),
                "dev",
                "lan0",
            ]);
            run(&["ip", "-n", &nat, "link", "set", "lan0", "up"]);
            run(&[
                "ip",
                "-n",
                &host,
                "addr",
                "add",
                &format!("{net}.2/24"),
                "dev",
                "eth0",
            ]);
            run(&["ip", "-n", &host, "link", "set", "eth0", "up"]);
            run(&[
                "ip",
                "-n",
                &host,
                "route",
                "add",
                "default",
                "via",
                &format!("{net}.1"),
            ]);
            let exec = |args: &[&str]| {
                let mut all = vec!["ip", "netns", "exec", nat.as_str()];
                all.extend_from_slice(args);
                run(&all);
            };
            exec(&["sysctl", "-qw", "net.ipv4.ip_forward=1"]);
            exec(&[
                "iptables",
                "-t",
                "nat",
                "-A",
                "POSTROUTING",
                "-o",
                "wan0",
                "-j",
                "MASQUERADE",
            ]);
            // A home router: nothing unsolicited from the WAN, to itself or to the LAN.
            exec(&[
                "iptables",
                "-A",
                "INPUT",
                "-i",
                "wan0",
                "-m",
                "conntrack",
                "--ctstate",
                "NEW",
                "-j",
                "DROP",
            ]);
            exec(&[
                "iptables",
                "-A",
                "FORWARD",
                "-i",
                "wan0",
                "-m",
                "conntrack",
                "--ctstate",
                "NEW",
                "-j",
                "DROP",
            ]);
        }
        Lab
    }

    fn clean() {
        for ns in NAMESPACES {
            let _ = Command::new("ip")
                .args(["netns", "del", ns])
                .stderr(Stdio::null())
                .status();
        }
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        Self::clean();
    }
}

fn spawn_role(ns: &str, role: &str, dir: &Path) -> Child {
    let exe = std::env::current_exe().expect("test binary path");
    Command::new("ip")
        .args(["netns", "exec", ns])
        .arg(exe)
        .args(["nat_lab_child", "--exact", "--nocapture", "--test-threads=1"])
        .env("WASL_NAT_LAB_ROLE", role)
        .env("WASL_NAT_LAB_DIR", dir)
        .spawn()
        .expect("start a lab device")
}

fn wait(mut child: Child, what: &str) {
    let deadline = Instant::now() + CHILD_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("wait") {
            assert!(status.success(), "{what} failed: {status}");
            return;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("{what} did not finish in {CHILD_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn nat_lab() {
    if std::env::var("WASL_NAT_LAB").as_deref() != Ok("1") || !is_root() {
        eprintln!("nat_lab skipped: needs root and WASL_NAT_LAB=1 (see .github/workflows/nat-lab.yml)");
        return;
    }
    let _lab = Lab::build();
    let dir = std::env::temp_dir().join(format!("wasl-nat-lab-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut stun_server = spawn_role("wl-wan", "stun", &dir);
    std::thread::sleep(Duration::from_millis(500));
    let b = spawn_role("wl-b", "b", &dir);
    let a = spawn_role("wl-a", "a", &dir);
    wait(a, "device a");
    wait(b, "device b");
    let _ = stun_server.kill();
    let _ = stun_server.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// One lab role, run inside its namespace by [`nat_lab`]. Does nothing in a normal test run.
#[test]
fn nat_lab_child() {
    let Ok(role) = std::env::var("WASL_NAT_LAB_ROLE") else {
        return;
    };
    let dir = PathBuf::from(std::env::var("WASL_NAT_LAB_DIR").expect("lab dir"));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        match role.as_str() {
            "stun" => {
                let socket = tokio::net::UdpSocket::bind(STUN_ADDR).await.unwrap();
                stun::respond(socket).await.unwrap();
            }
            "a" => device(true, &dir).await,
            "b" => device(false, &dir).await,
            other => panic!("unknown role {other}"),
        }
    });
}

struct Echo;

#[async_trait]
impl PeerService for Echo {
    async fn shared(&self, _: DeviceId) -> Vec<ShareId> {
        vec![ShareId([7; 16])]
    }
    async fn changes_since(&self, _: DeviceId, _: ShareId, _: u64, _: usize) -> PeerResult<IndexPage> {
        Err(PeerError::NotShared)
    }
    async fn chunk_list(&self, _: DeviceId, _: ShareId, _: &str, _: Hash) -> PeerResult<ChunkList> {
        Err(PeerError::NotShared)
    }
    async fn chunk(&self, _: DeviceId, _: ShareId, _: Hash) -> PeerResult<Vec<u8>> {
        Err(PeerError::NotShared)
    }
    async fn notify(&self, _: DeviceId, _: ShareId, _: u64) {}
}

/// Device `a` dials, device `b` answers.
async fn device(dials: bool, dir: &Path) {
    let a = DeviceKey::from_seed(&[1; 32]);
    let b = DeviceKey::from_seed(&[2; 32]);
    let (me, peer) = if dials { (&a, &b) } else { (&b, &a) };
    let name = if dials { "a" } else { "b" };
    let pks = [a.public(), b.public()];
    let trust: Trust = Arc::new(move |pk| pks.contains(pk));
    let ep = Endpoint::bind("0.0.0.0:0".parse().unwrap(), me, trust.clone()).unwrap();
    ep.serve(Arc::new(Echo));
    let stun_server: SocketAddr = STUN_ADDR.parse().unwrap();
    let ours = holepunch::gather(
        &ep,
        Reach::LanAndInternet,
        &[stun_server],
        None,
        &StunConfig::default(),
    )
    .await;
    eprintln!("[{name}] local {:?}, publishing {ours:?}", ep.local_addr());
    let public = ours.stun.expect("STUN gave a public address");
    assert_ne!(public.ip(), ep.local_addr().unwrap().ip(), "behind a NAT");
    let rv = DirRendezvous(dir.to_path_buf());
    let record = PeerRecord::new(me, ours.clone(), rendezvous::unix_now(), Duration::from_secs(600)).unwrap();
    rendezvous::publish(&rv, &record).await.unwrap();
    let cfg = PunchConfig::default();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    if dials {
        // Wait for b to be up.
        while rendezvous::fetch_record(&rv, peer.id(), trust.as_ref(), rendezvous::unix_now())
            .await
            .unwrap()
            .is_none()
        {
            assert!(tokio::time::Instant::now() < deadline, "b never published");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let (link, via) = holepunch::dial(
            &ep,
            Reach::LanAndInternet,
            &rv,
            me,
            &ours,
            peer.id(),
            trust.as_ref(),
            &cfg,
        )
        .await
        .expect("a reaches b");
        eprintln!("[a] connected to {} via {via:?}", link.remote_addr());
        assert_eq!(via.via, Via::Punch);
        assert_eq!(link.peer(), peer.id());
        assert_eq!(link.shared().await.unwrap(), vec![ShareId([7; 16])]);
        // Tell b it may stop, then leave it time to see that.
        std::fs::write(dir.join("a-done"), b"").unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
    } else {
        let mut guard = ReplayGuard::new(256);
        let mut limits = holepunch::AnswerLimits::default();
        loop {
            assert!(tokio::time::Instant::now() < deadline, "no offer reached b");
            let got = holepunch::answer(
                &ep,
                Reach::LanAndInternet,
                &rv,
                me,
                &ours,
                trust.as_ref(),
                &|_: &wasl_net::DeviceId| true,
                &mut guard,
                &mut limits,
                &cfg,
            )
            .await
            .unwrap();
            if let Some((from, result)) = got.into_iter().next() {
                let (link, via) = result.expect("b reaches a");
                eprintln!("[b] connected to {} via {via:?}", link.remote_addr());
                assert_eq!(from, peer.id());
                assert_eq!(via.via, Via::Punch);
                assert_eq!(link.shared().await.unwrap(), vec![ShareId([7; 16])]);
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        // Both sides dial at once and either may finish first. A real device stays up; here b
        // keeps its endpoint until a's own connection is through, so a's retries (sent before
        // b's router had a hole for them) still find someone to answer.
        while !dir.join("a-done").exists() {
            assert!(tokio::time::Instant::now() < deadline, "a never finished");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

/// A rendezvous in a directory both lab devices can reach.
struct DirRendezvous(PathBuf);

fn storage(e: std::io::Error) -> RendezvousError {
    RendezvousError::Storage(e.to_string())
}

#[async_trait]
impl Rendezvous for DirRendezvous {
    async fn put(&self, name: &str, bytes: Vec<u8>) -> Result<(), RendezvousError> {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(storage)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes).map_err(storage)?;
        std::fs::rename(&tmp, &path).map_err(storage)
    }

    async fn get(&self, name: &str) -> Result<Option<Vec<u8>>, RendezvousError> {
        match std::fs::read(self.0.join(name)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(storage(e)),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, RendezvousError> {
        let (folder, start) = prefix.rsplit_once('/').unwrap_or(("", prefix));
        let Ok(entries) = std::fs::read_dir(self.0.join(folder)) else {
            return Ok(Vec::new());
        };
        Ok(entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.starts_with(start) && n.ends_with(".json"))
            .map(|n| {
                if folder.is_empty() {
                    n
                } else {
                    format!("{folder}/{n}")
                }
            })
            .collect())
    }

    async fn delete(&self, name: &str) -> Result<(), RendezvousError> {
        match std::fs::remove_file(self.0.join(name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(storage(e)),
            _ => Ok(()),
        }
    }
}
