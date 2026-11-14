//! The socket: a Unix domain socket (0600, in a 0700 folder) or a Windows named pipe, carrying one
//! JSON value per line.

use crate::paths::Paths;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use zeroize::Zeroize;

/// The longest line accepted: a request or a whole history page, never a file.
pub const MAX_LINE: usize = 32 << 20;

/// The longest line accepted before the handshake: a hello or a proof is under 200 bytes. Any
/// process of any user may open the pipe, so before it has proven it knows the token it must not
/// make the daemon buffer up to [`MAX_LINE`] (security review 2026-10).
pub const MAX_HANDSHAKE_LINE: usize = 4 << 10;

pub trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}

pub type BoxStream = Box<dyn Duplex>;

pub struct LineReader {
    inner: BufReader<ReadHalf<BoxStream>>,
    buf: Vec<u8>,
    limit: usize,
}

pub struct LineWriter {
    inner: WriteHalf<BoxStream>,
}

pub fn split(stream: BoxStream) -> (LineReader, LineWriter) {
    let (r, w) = tokio::io::split(stream);
    (
        LineReader {
            inner: BufReader::new(r),
            buf: Vec::new(),
            limit: MAX_LINE,
        },
        LineWriter { inner: w },
    )
}

impl LineReader {
    /// Sets the longest line accepted from now on ([`MAX_LINE`] by default).
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
    }

    /// The next value, or None at end of stream.
    ///
    /// A line can carry a vault passphrase (unlocking or creating a vault), so the line buffer is
    /// wiped once the value is parsed or refused, and never left behind when it grows (security
    /// review S4-5). The parsed value owns its own copy: [`crate::Passphrase`] wipes that one.
    pub async fn read<T: DeserializeOwned>(&mut self) -> io::Result<Option<T>> {
        let result = self.read_line().await;
        self.buf.zeroize();
        result
    }

    async fn read_line<T: DeserializeOwned>(&mut self) -> io::Result<Option<T>> {
        self.buf.clear();
        loop {
            let available = self.inner.fill_buf().await?;
            if available.is_empty() {
                return if self.buf.is_empty() {
                    Ok(None)
                } else {
                    Err(io::Error::new(io::ErrorKind::UnexpectedEof, "line cut short"))
                };
            }
            let (take, done) = match available.iter().position(|&b| b == b'\n') {
                Some(i) => (i + 1, true),
                None => (available.len(), false),
            };
            extend_wiping(&mut self.buf, &available[..take]);
            self.inner.consume(take);
            if self.buf.len() > self.limit {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "message too long"));
            }
            if done {
                let line = self.buf.strip_suffix(b"\n").unwrap_or(&self.buf);
                return serde_json::from_slice(line)
                    .map(Some)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
            }
        }
    }
}

impl LineWriter {
    pub async fn write<T: Serialize>(&mut self, value: &T) -> io::Result<()> {
        let mut line =
            serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        line.push(b'\n');
        let sent = self.inner.write_all(&line).await;
        // What a client sends can carry a passphrase too (S4-5).
        line.zeroize();
        sent?;
        self.inner.flush().await
    }
}

/// Appends `data`, wiping the old allocation when `buf` has to move to a bigger one (a plain
/// `extend` would free it with its bytes still in it).
fn extend_wiping(buf: &mut Vec<u8>, data: &[u8]) {
    if buf.capacity() - buf.len() < data.len() {
        let mut bigger = Vec::with_capacity((buf.len() + data.len()).max(buf.capacity() * 2));
        bigger.extend_from_slice(buf);
        buf.zeroize();
        *buf = bigger;
    }
    buf.extend_from_slice(data);
}

/// Connects to the daemon of `paths`.
pub async fn connect(paths: &Paths) -> io::Result<BoxStream> {
    #[cfg(unix)]
    {
        let s = tokio::net::UnixStream::connect(paths.socket()).await?;
        Ok(Box::new(s))
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        let name = paths.pipe_name();
        loop {
            match ClientOptions::new().open(&name) {
                Ok(c) => return Ok(Box::new(c)),
                // All instances busy: the daemon makes a new one at once.
                Err(e) if e.raw_os_error() == Some(231) => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// The daemon's end.
pub struct Listener {
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
    #[cfg(windows)]
    name: String,
    #[cfg(windows)]
    next: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
}

impl Listener {
    /// Binds the socket. Fails if another daemon of this user already listens.
    pub fn bind(paths: &Paths) -> io::Result<Listener> {
        paths.ensure()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = paths.socket();
            if path.exists() {
                // A live daemon answers; a dead one left its socket behind.
                if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "wasld is already running",
                    ));
                }
                std::fs::remove_file(&path)?;
            }
            let inner = tokio::net::UnixListener::bind(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Listener { inner })
        }
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            let name = paths.pipe_name();
            // The first instance must be ours: if the name exists, someone else holds it.
            //
            // The pipe keeps Windows' default access list (the owner, SYSTEM and administrators
            // get full access; other users may only open it for reading, so they cannot send a
            // request). A narrower list would need unsafe Win32 calls, which this workspace
            // forbids; the handshake (crate::auth) already refuses any process that cannot read
            // this user's token file before it can make a single request.
            let first = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(&name)
                .map_err(|e| {
                    if e.kind() == io::ErrorKind::PermissionDenied {
                        io::Error::new(io::ErrorKind::AddrInUse, "wasld is already running")
                    } else {
                        e
                    }
                })?;
            Ok(Listener {
                name,
                next: Some(first),
            })
        }
    }

    pub async fn accept(&mut self) -> io::Result<BoxStream> {
        #[cfg(unix)]
        {
            let (s, _) = self.inner.accept().await?;
            Ok(Box::new(s))
        }
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            let server = match self.next.take() {
                Some(s) => s,
                None => ServerOptions::new()
                    .reject_remote_clients(true)
                    .create(&self.name)?,
            };
            // A listening instance always exists, even while this one connects or fails, so the
            // name is never free for another process to take.
            self.next = Some(
                ServerOptions::new()
                    .reject_remote_clients(true)
                    .create(&self.name)?,
            );
            server.connect().await?;
            Ok(Box::new(server))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::extend_wiping;

    #[test]
    fn growing_the_line_buffer_keeps_its_bytes_in_order() {
        let mut buf = Vec::with_capacity(2);
        for part in [&b"ab"[..], b"cde", b"", b"fghijklmnop"] {
            extend_wiping(&mut buf, part);
        }
        assert_eq!(b"abcdefghijklmnop".to_vec(), buf);
    }
}
