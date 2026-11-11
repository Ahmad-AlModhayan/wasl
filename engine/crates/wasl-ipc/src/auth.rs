//! The handshake. The OS keeps other users out of the socket (a 0600 Unix socket, a pipe in the
//! user's session); on top of that both sides prove they know the token in the user's data folder,
//! without sending it. So a client never talks to a process that squatted the pipe name, and a
//! process that cannot read the user's files cannot drive the daemon.
//!
//! client → `{"hello":{"version":1,"nonce":…}}`
//! daemon → `{"proof":HMAC(token,"wasld"‖client nonce),"nonce":…}`
//! client → `{"proof":HMAC(token,"wasl"‖daemon nonce)}`
//! daemon → `{"welcome":{"version":…}}` or closes.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::io;
use std::path::Path;

pub(crate) const NONCE_LEN: usize = 32;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Hello {
    pub version: u32,
    pub nonce: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DaemonProof {
    pub proof: String,
    pub nonce: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ClientProof {
    pub proof: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Welcome {
    pub version: String,
    pub protocol: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Opening {
    #[serde(rename = "hello")]
    Hello(Hello),
}

/// The token: 32 random bytes, base64.
#[derive(Clone)]
pub struct Token(Vec<u8>);

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(…)")
    }
}

impl Token {
    /// A new token, written to `path` (this user only). The daemon makes one at every start.
    pub fn create(path: &Path) -> io::Result<Token> {
        let mut bytes = vec![0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
        let tmp = path.with_extension("tmp");
        // On Windows the file is limited to this user before the secret goes in, so it is never
        // readable by others even for a moment, whatever the folder's access list
        // (security review 2026-10).
        #[cfg(windows)]
        {
            std::fs::write(&tmp, b"")?;
            crate::paths::user_only(&tmp, false)?;
        }
        std::fs::write(&tmp, B64.encode(&bytes))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(Token(bytes))
    }

    pub fn read(path: &Path) -> io::Result<Token> {
        let text = std::fs::read_to_string(path)?;
        let bytes = B64
            .decode(text.trim())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the IPC token is damaged"))?;
        if bytes.len() != 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the IPC token is damaged",
            ));
        }
        Ok(Token(bytes))
    }

    pub(crate) fn proof(&self, role: &str, nonce: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("any key length");
        mac.update(role.as_bytes());
        mac.update(b"\0");
        mac.update(nonce.as_bytes());
        B64.encode(mac.finalize().into_bytes())
    }

    /// Constant-time comparison of a proof.
    pub(crate) fn verify(&self, role: &str, nonce: &str, proof: &str) -> bool {
        let Ok(given) = B64.decode(proof) else {
            return false;
        };
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("any key length");
        mac.update(role.as_bytes());
        mac.update(b"\0");
        mac.update(nonce.as_bytes());
        mac.verify_slice(&given).is_ok()
    }
}

pub(crate) fn nonce() -> io::Result<String> {
    let mut b = [0u8; NONCE_LEN];
    getrandom::fill(&mut b).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(B64.encode(b))
}

pub(crate) const DAEMON_ROLE: &str = "wasld";
pub(crate) const CLIENT_ROLE: &str = "wasl";
