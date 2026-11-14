//! The daemon side: accepts this user's clients, checks the handshake, and runs each request on
//! the [`Handler`] concurrently. Subscribed connections get every [`Event`] sent to [`Server::events`].

use crate::auth::{self, ClientProof, DaemonProof, Opening, Token, Welcome};
use crate::paths::Paths;
use crate::proto::{Event, Frame, PROTOCOL_VERSION, RemoteError, Request};
use crate::transport::{self, BoxStream, Listener};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, broadcast};

/// Answers requests. `Subscribe` is handled by the server itself.
#[async_trait::async_trait]
pub trait Handler: Send + Sync + 'static {
    async fn handle(&self, request: Request) -> Result<serde_json::Value, RemoteError>;
}

pub struct Server<H: Handler> {
    listener: Listener,
    token: Token,
    handler: Arc<H>,
    events: broadcast::Sender<Event>,
    version: String,
    /// Connections that have not finished the handshake yet.
    unproven: Arc<Semaphore>,
}

/// How many connections may be in the handshake at once. Any local process can open the pipe;
/// without a cap, one that opens thousands and never speaks holds a task and a buffer for each
/// until the handshake timeout (security review 2026-10). This user's own clients finish the
/// handshake in milliseconds, so they rarely need more than one or two.
pub const MAX_UNPROVEN: usize = 16;

/// How long a client may take to finish the handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

impl<H: Handler> Server<H> {
    /// Binds the socket and writes a fresh token. Fails if this user's daemon already runs.
    pub fn bind(paths: &Paths, handler: Arc<H>, version: impl Into<String>) -> io::Result<Server<H>> {
        let listener = Listener::bind(paths).map_err(|e| in_use(paths, e))?;
        let token = Token::create(&paths.token())?;
        let (events, _) = broadcast::channel(1024);
        Ok(Server {
            listener,
            token,
            handler,
            events,
            version: version.into(),
            unproven: Arc::new(Semaphore::new(MAX_UNPROVEN)),
        })
    }

    /// Sends events to subscribed clients. Cheap to clone and keep.
    pub fn events(&self) -> broadcast::Sender<Event> {
        self.events.clone()
    }

    /// Serves until the task is dropped.
    pub async fn serve(mut self) -> io::Result<()> {
        loop {
            // One failed accept (a client that hung up mid-connect, a busy system) must never stop
            // the daemon listening: wait a moment and take the next client.
            let stream = match self.listener.accept().await {
                Ok(s) => s,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            // Over the cap: close at once rather than queue, so connections that never speak
            // cannot make this user's clients wait behind them.
            let Ok(permit) = self.unproven.clone().try_acquire_owned() else {
                drop(stream);
                continue;
            };
            let token = self.token.clone();
            let handler = self.handler.clone();
            let events = self.events.clone();
            let version = self.version.clone();
            tokio::spawn(async move {
                let _ = connection(stream, permit, token, handler, events, version).await;
            });
        }
    }
}

async fn connection<H: Handler>(
    stream: BoxStream,
    permit: OwnedSemaphorePermit,
    token: Token,
    handler: Arc<H>,
    events: broadcast::Sender<Event>,
    version: String,
) -> io::Result<()> {
    let (mut reader, mut writer) = transport::split(stream);
    reader.set_limit(transport::MAX_HANDSHAKE_LINE);
    let refused = || io::Error::new(io::ErrorKind::PermissionDenied, "handshake refused");

    let handshake = async {
        let Opening::Hello(hello) = reader.read::<Opening>().await?.ok_or_else(refused)?;
        let mine = auth::nonce()?;
        writer
            .write(&DaemonProof {
                proof: token.proof(auth::DAEMON_ROLE, &hello.nonce),
                nonce: mine.clone(),
            })
            .await?;
        let theirs: ClientProof = reader.read().await?.ok_or_else(refused)?;
        if !token.verify(auth::CLIENT_ROLE, &mine, &theirs.proof) {
            return Err(refused());
        }
        writer
            .write(&Welcome {
                version: version.clone(),
                protocol: PROTOCOL_VERSION,
            })
            .await?;
        Ok::<(), io::Error>(())
    };
    tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "handshake too slow"))??;
    // Proven: this user's client, which may send whole requests and hold the connection open.
    drop(permit);
    reader.set_limit(transport::MAX_LINE);

    let writer = Arc::new(Mutex::new(writer));
    while let Some(frame) = reader.read::<Frame>().await? {
        let Frame::Request { id, request } = frame else {
            continue;
        };
        if matches!(*request, Request::Subscribe) {
            let mut rx = events.subscribe();
            let w = writer.clone();
            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(event) => {
                            if w.lock().await.write(&Frame::Event { event }).await.is_err() {
                                break;
                            }
                        }
                        // A slow client misses events; it re-reads on the next Changed.
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            if w.lock()
                                .await
                                .write(&Frame::Event {
                                    event: Event::Changed,
                                })
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            writer
                .lock()
                .await
                .write(&Frame::Ok {
                    id,
                    ok: serde_json::Value::Null,
                })
                .await?;
            continue;
        }
        let handler = handler.clone();
        let w = writer.clone();
        tokio::spawn(async move {
            let frame = match handler.handle(*request).await {
                Ok(ok) => Frame::Ok { id, ok },
                Err(err) => Frame::Err { id, err },
            };
            let _ = w.lock().await.write(&frame).await;
        });
    }
    Ok(())
}

/// Explains why the pipe could not be made. On Windows the name is per user and data folder, so
/// if it exists, either this user's daemon already runs or some other program squatted the name;
/// telling them apart matters, because "already running" sends the user looking for a daemon
/// that is not there (security review 2026-10). The owner is asked to prove it knows this user's
/// token, as every client asks.
fn in_use(paths: &Paths, e: io::Error) -> io::Error {
    if e.kind() != io::ErrorKind::AddrInUse {
        return e;
    }
    #[cfg(windows)]
    {
        if !owner_is_wasld(paths) {
            return io::Error::new(
                io::ErrorKind::AddrInUse,
                format!(
                    "the pipe name {} is in use by another program, so wasld cannot listen",
                    paths.pipe_name()
                ),
            );
        }
    }
    #[cfg(not(windows))]
    let _ = paths;
    e
}

/// Whether the process holding the pipe name answers the handshake with this user's token. Runs
/// on its own thread with a deadline: the owner may be a program that never answers, and `bind`
/// is synchronous.
#[cfg(windows)]
fn owner_is_wasld(paths: &Paths) -> bool {
    let Ok(token) = Token::read(&paths.token()) else {
        return false;
    };
    let name = paths.pipe_name();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(probe(&name, &token).unwrap_or(false));
    });
    rx.recv_timeout(HANDSHAKE_TIMEOUT).unwrap_or(false)
}

#[cfg(windows)]
fn probe(name: &str, token: &Token) -> io::Result<bool> {
    use crate::auth::Hello;
    use std::io::{BufRead, BufReader, Read, Write};
    let pipe = loop {
        match std::fs::OpenOptions::new().read(true).write(true).open(name) {
            Ok(p) => break p,
            // All instances busy: a live daemon makes a new one at once.
            Err(e) if e.raw_os_error() == Some(231) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    };
    let mine = auth::nonce()?;
    let mut hello = serde_json::to_vec(&Opening::Hello(Hello {
        version: PROTOCOL_VERSION,
        nonce: mine.clone(),
    }))
    .map_err(io::Error::other)?;
    hello.push(b'\n');
    (&pipe).write_all(&hello)?;
    let mut line = String::new();
    BufReader::new((&pipe).take(transport::MAX_HANDSHAKE_LINE as u64)).read_line(&mut line)?;
    let Ok(theirs) = serde_json::from_str::<DaemonProof>(line.trim_end()) else {
        return Ok(false);
    };
    Ok(token.verify(auth::DAEMON_ROLE, &mine, &theirs.proof))
}
