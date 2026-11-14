//! v2 names (ON_DEMAND.md §1.2).
//!
//! ```text
//! encrypted = base64url-no-padding(AES-256-SIV(kName, nameUtf8, ad = [parent dir id (16)])) + ".w2"
//! ```
//!
//! AES-SIV (RFC 5297) with one associated-data component, output `V (16) || C`. Deterministic in
//! its parent, so the Drive duplicate-name check keeps working, and bound to the parent: an item
//! moved into another folder no longer decrypts. Every folder has a random [`DirId`] (the vault
//! root's is all zeros), kept in the folder's `appProperties["wasl.dir"]` and, for recovery from a
//! plain Drive download (which drops appProperties), in a `.wasl-dir` object inside the folder
//! ([`dir_marker`]). Both are authenticated and bound to the folder's [`Placement`]:
//!
//! ```text
//! tag        = HMAC-SHA256(kDir, "wasl-v2-dir" || parent dir id (16) || folder name SIV bytes || dir id)
//! .wasl-dir  = "W2D1" || dir id (16) || tag (32)
//! wasl.dir   = base64url(dir id (16) || tag (32))                     (64 characters)
//! ```
//!
//! A [`Placement`] (parent dir id || the item's name as SIV bytes, i.e. the encrypted name before
//! base64url) is also what file manifests and `wasl.m` blobs are bound to, so none of these can be
//! moved to another item undetected.

use super::{B64URL, keys::VaultKeysV2};
use crate::error::{CryptoError, Result};
use crate::kdf::hmac_sha256;
use aes_siv::KeyInit;
use aes_siv::siv::Aes256Siv;
use base64::Engine as _;

pub const SUFFIX: &str = ".w2";
pub const SIV_BYTES: usize = 16;
pub const DIR_ID_BYTES: usize = 16;
/// The Drive appProperties key holding a folder's authenticated dir id (base64url, 64 characters).
pub const DIR_PROPERTY: &str = "wasl.dir";
/// The object inside every encrypted folder (not the root) that repeats its authenticated dir id.
pub const DIR_MARKER_NAME: &str = ".wasl-dir";
pub const DIR_MARKER_MAGIC: [u8; 4] = *b"W2D1";
pub const DIR_TAG_LABEL: &str = "wasl-v2-dir";
pub const DIR_TAG_BYTES: usize = 32;
/// The `.wasl-dir` object's fixed size: magic, dir id, tag.
pub const DIR_MARKER_BYTES: usize = DIR_MARKER_MAGIC.len() + DIR_ID_BYTES + DIR_TAG_BYTES;

/// A folder's random 16-byte id, the associated data of its children's names.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DirId(pub [u8; DIR_ID_BYTES]);

impl DirId {
    /// The vault root's id: all zeros.
    pub const ROOT: DirId = DirId([0; DIR_ID_BYTES]);

    /// A fresh id from the OS random number generator.
    pub fn random() -> Result<DirId> {
        super::random().map(DirId)
    }

    pub fn as_bytes(&self) -> &[u8; DIR_ID_BYTES] {
        &self.0
    }

    /// The value of the folder's `appProperties["wasl.dir"]`: base64url(id || tag), bound to the
    /// folder's placement (`folder`: the folder's own parent and name).
    pub fn to_property(&self, keys: &VaultKeysV2, folder: &Placement) -> String {
        let mut raw = self.0.to_vec();
        raw.extend_from_slice(&dir_tag(keys, folder, self));
        B64URL.encode(raw)
    }

    /// The id in a `wasl.dir` value, `None` unless it verifies for this folder's placement.
    pub fn from_property(keys: &VaultKeysV2, folder: &Placement, s: &str) -> Option<DirId> {
        let raw = B64URL.decode(s).ok()?;
        verify_id_and_tag(keys, folder, &raw)
    }
}

/// Where an item lives: its parent folder's id and its own encrypted name (the AES-SIV output,
/// before base64url). File manifests, `wasl.m` blobs and folder ids are bound to it.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Placement {
    pub parent: DirId,
    pub name_siv: Vec<u8>,
}

impl Placement {
    /// From an encrypted name as listed (`<base64url>.w2`); `None` when it is not one.
    pub fn from_encrypted(parent: &DirId, encrypted: &str) -> Option<Placement> {
        let body = encrypted.strip_suffix(SUFFIX)?;
        let raw = B64URL.decode(body).ok()?;
        (raw.len() > SIV_BYTES).then_some(Placement {
            parent: *parent,
            name_siv: raw,
        })
    }

    /// For a plaintext name in `parent` (encrypts the name).
    pub fn for_name(keys: &VaultKeysV2, parent: &DirId, name: &str) -> Result<Placement> {
        let enc = encrypt_name(keys, parent, name)?;
        Placement::from_encrypted(parent, &enc)
            .ok_or_else(|| CryptoError::InvalidInput("name did not encrypt".into()))
    }

    /// parent dir id (16) || name SIV bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(DIR_ID_BYTES + self.name_siv.len());
        v.extend_from_slice(self.parent.as_bytes());
        v.extend_from_slice(&self.name_siv);
        v
    }
}

impl std::fmt::Debug for Placement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Placement({:?}, {})",
            self.parent,
            B64URL.encode(&self.name_siv)
        )
    }
}

fn dir_tag(keys: &VaultKeysV2, folder: &Placement, id: &DirId) -> [u8; DIR_TAG_BYTES] {
    let mut msg = DIR_TAG_LABEL.as_bytes().to_vec();
    msg.extend_from_slice(&folder.to_bytes());
    msg.extend_from_slice(id.as_bytes());
    hmac_sha256(keys.k_dir(), &msg)
}

fn verify_id_and_tag(keys: &VaultKeysV2, folder: &Placement, raw: &[u8]) -> Option<DirId> {
    if raw.len() != DIR_ID_BYTES + DIR_TAG_BYTES {
        return None;
    }
    let id = DirId(raw[..DIR_ID_BYTES].try_into().ok()?);
    let expected = dir_tag(keys, folder, &id);
    let diff = expected
        .iter()
        .zip(&raw[DIR_ID_BYTES..])
        .fold(0u8, |a, (x, y)| a | (x ^ y));
    (std::hint::black_box(diff) == 0).then_some(id)
}

impl std::fmt::Debug for DirId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DirId({})", super::hex(&self.0))
    }
}

/// True when `name` has the shape of a v2 vault name (suffix only).
pub fn is_vault_name(name: &str) -> bool {
    name.len() > SUFFIX.len() && name.ends_with(SUFFIX)
}

/// Whether a plaintext name may be stored: not empty, not `.` or `..`, no `/` or NUL.
pub fn is_valid_plain_name(name: &str) -> bool {
    !(name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0'))
}

/// Encrypts `name` for the folder whose id is `parent`.
pub fn encrypt_name(keys: &VaultKeysV2, parent: &DirId, name: &str) -> Result<String> {
    if !is_valid_plain_name(name) {
        return Err(CryptoError::InvalidInput(format!(
            "\"{}\" cannot be a vault name (empty, \".\", \"..\", or contains '/' or NUL)",
            name.escape_debug()
        )));
    }
    let raw = siv(keys)
        .encrypt([parent.as_bytes()], name.as_bytes())
        .map_err(|_| CryptoError::InvalidInput("name is too long to encrypt".into()))?;
    Ok(B64URL.encode(raw) + SUFFIX)
}

/// The plaintext name, or `None` when `encrypted` is not a valid name of these keys in the folder
/// `parent` (foreign file, moved, tampered, malformed). Never panics on hostile input.
pub fn decrypt_name(keys: &VaultKeysV2, parent: &DirId, encrypted: &str) -> Option<String> {
    let body = encrypted.strip_suffix(SUFFIX)?;
    if body.is_empty()
        || !body
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return None;
    }
    let raw = B64URL.decode(body).ok()?;
    if raw.len() <= SIV_BYTES {
        return None;
    }
    let plain = siv(keys).decrypt([parent.as_bytes()], &raw).ok()?;
    let name = String::from_utf8(plain).ok()?;
    is_valid_plain_name(&name).then_some(name)
}

/// The `.wasl-dir` object's content: `"W2D1" || dir id (16) || tag (32)`, for the folder at
/// `folder`.
pub fn dir_marker(keys: &VaultKeysV2, folder: &Placement, id: &DirId) -> Vec<u8> {
    let mut v = DIR_MARKER_MAGIC.to_vec();
    v.extend_from_slice(id.as_bytes());
    v.extend_from_slice(&dir_tag(keys, folder, id));
    v
}

/// The dir id in a `.wasl-dir` object, or `None` when it is not one or does not verify for the
/// folder at `folder` (tampered, or copied from another folder).
pub fn parse_dir_marker(keys: &VaultKeysV2, folder: &Placement, bytes: &[u8]) -> Option<DirId> {
    let rest = bytes.strip_prefix(&DIR_MARKER_MAGIC[..])?;
    verify_id_and_tag(keys, folder, rest)
}

fn siv(keys: &VaultKeysV2) -> Aes256Siv {
    Aes256Siv::new(keys.k_name().into())
}
