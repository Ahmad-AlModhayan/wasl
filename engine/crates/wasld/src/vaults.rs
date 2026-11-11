//! Encrypted vaults on this computer (docs/ENCRYPTION.md, v1 and §11 v2): making a new vault
//! (always format v2), unlocking one that exists (v1 or v2), locking it again, and the remotes a
//! vault's pairs sync through and its views read through.
//!
//! Where things are kept:
//! * the vault's public description (its `vault.json`: salt, cost, key check, name) and the Drive
//!   folder it was found in, in the store's `vaults` table: no secret;
//! * while it is unlocked here, its key material in the OS keychain under the service
//!   [`KEYCHAIN_SERVICE`], one entry per vault id: v1 keeps keyEnc ‖ keyName (64 bytes), v2 the
//!   Argon2id output (32 bytes), as the phone does. It is read when a pair runs or a view mounts,
//!   checked against the key check, and wiped from memory when dropped. Locking deletes it;
//! * the passphrase nowhere. It reaches the daemon over the local socket (only this OS user's
//!   clients can connect), the keys are derived off the async threads, and it is wiped.

use crate::tokens::Secrets;
use crate::views::{VAULT_LOCKED, ViewRemote, ViewRemotes};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use wasl_crypto::v2::Format;
use wasl_crypto::v2::{KdfParams, VaultKeysV2, keys as v2keys, vault_json::VaultManifestV2};
use wasl_crypto::{CryptoError, VaultCipher, VaultKeys, VaultManifest};
use wasl_engine::{
    CopyReport, EngineError, Remote, RemoteSource, VaultRemote, VaultV1Source, VaultV2Remote, copy_vault,
};
use wasl_ipc::{ErrorKind, Passphrase, RemoteError, VaultFolder, VaultInfo, View};
use wasl_store::{Pair, SharedStore, VaultRecord};
use wasl_vfs::{FileRef, Source, VfsError};
use zeroize::Zeroizing;

/// The keychain service vault keys live under (OAuth tokens and the device key use "Wasl").
pub const KEYCHAIN_SERVICE: &str = "Wasl vaults";
/// What a wrong passphrase says.
pub const WRONG_PASSPHRASE: &str = "Wrong passphrase for this vault.";
/// The shortest passphrase a new vault takes (as on the phone).
pub const MIN_PASSPHRASE_CHARS: usize = 8;
/// How long linking a new pair waits to learn whether its Drive folder is a vault.
const LINK_PAIR_WITHIN: std::time::Duration = std::time::Duration::from_secs(20);
pub use wasl_engine::plain_guard::{
    DAMAGED_MANIFEST, PLAIN_ON_DAMAGED_VAULT, PLAIN_ON_VAULT, manifest_text, plain_on_nested_vault,
};

/// Unlocked key material with the `vault.json` it belongs to.
pub enum Keys {
    V1 {
        keys: Arc<VaultKeys>,
        manifest: VaultManifest,
    },
    V2 {
        keys: Arc<VaultKeysV2>,
        manifest: VaultManifestV2,
    },
}

/// A `vault.json` of either format.
#[derive(Clone)]
enum Manifest {
    V1(VaultManifest),
    V2(VaultManifestV2),
}

impl Manifest {
    /// `None` only when the text is definitely not a Wasl vault's: a JSON object naming another
    /// `format`. Anything else that is not a usable Wasl manifest (not JSON, no format, a Wasl
    /// format that does not parse) is an error: it may be a damaged vault, and is never taken for
    /// a plain file.
    fn parse(text: &str) -> Result<Option<Manifest>, CryptoError> {
        let damaged = || CryptoError::Integrity(DAMAGED_MANIFEST.into());
        let value: serde_json::Value = serde_json::from_str(text.trim()).map_err(|_| damaged())?;
        match value.get("format").and_then(|f| f.as_str()) {
            Some(VaultManifest::FORMAT) => VaultManifest::parse(text).map(|m| Some(Manifest::V1(m))),
            Some(VaultManifestV2::FORMAT) => VaultManifestV2::parse(text).map(|m| Some(Manifest::V2(m))),
            Some(_) => Ok(None),
            None => Err(damaged()),
        }
    }

    fn version(&self) -> u32 {
        match self {
            Manifest::V1(_) => 1,
            Manifest::V2(_) => 2,
        }
    }

    fn name(&self) -> &str {
        match self {
            Manifest::V1(m) => &m.name,
            Manifest::V2(m) => &m.name,
        }
    }

    fn json(&self) -> String {
        match self {
            Manifest::V1(m) => m.to_json(),
            Manifest::V2(m) => m.to_json(),
        }
    }

    fn key_check(&self) -> Vec<u8> {
        match self {
            Manifest::V1(m) => m.key_check_bytes().unwrap_or_default(),
            Manifest::V2(m) => m.key_check_bytes().map(|k| k.to_vec()).unwrap_or_default(),
        }
    }

    fn kdf_json(&self) -> String {
        match self {
            Manifest::V1(m) => serde_json::json!({"kdf": m.kdf, "iterations": m.iterations}),
            Manifest::V2(m) => serde_json::json!({"kdf": m.kdf, "m": m.m, "t": m.t, "p": m.p}),
        }
        .to_string()
    }

    fn created_ms(&self) -> i64 {
        match self {
            Manifest::V1(m) => m.created_ms(),
            Manifest::V2(m) => m.created_ms(),
        }
        .unwrap_or(0)
    }

    /// The vault's id on this computer: the same for every folder of the same vault (same format,
    /// salt, cost and key check), so unlocking a vault twice finds the same record.
    fn id(&self) -> String {
        let (format, salt, cost) = match self {
            Manifest::V1(m) => (m.format.as_str(), m.salt.as_str(), m.iterations.to_string()),
            Manifest::V2(m) => (
                m.format.as_str(),
                m.salt.as_str(),
                format!("{},{},{}", m.m, m.t, m.p),
            ),
        };
        let mut h = Sha256::new();
        for part in [
            b"wasl-vault-id".as_slice(),
            format.as_bytes(),
            salt.as_bytes(),
            cost.as_bytes(),
        ] {
            h.update(part);
            h.update([0u8]);
        }
        h.update(self.key_check());
        let hex: String = h.finalize()[..16].iter().map(|b| format!("{b:02x}")).collect();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }
}

/// Key material as derived, before it goes to the keychain.
enum Material {
    V1(VaultKeys),
    V2(Zeroizing<[u8; v2keys::IKM_BYTES]>),
}

impl Material {
    /// `<version>:<hex>`: the keychain entry.
    fn encode(&self) -> Zeroizing<String> {
        let (version, bytes): (u32, Zeroizing<Vec<u8>>) = match self {
            Material::V1(k) => {
                let mut b = Zeroizing::new(Vec::with_capacity(64));
                b.extend_from_slice(k.key_enc());
                b.extend_from_slice(k.key_name());
                (1, b)
            }
            Material::V2(ikm) => (2, Zeroizing::new(ikm.to_vec())),
        };
        let mut out = Zeroizing::new(format!("{version}:"));
        for b in bytes.iter() {
            out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
            out.push(char::from_digit(u32::from(b & 0xf), 16).unwrap_or('0'));
        }
        out
    }
}

fn decode_hex(hex: &str) -> Option<Zeroizing<Vec<u8>>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Zeroizing::new(Vec::with_capacity(hex.len() / 2));
    for pair in hex.as_bytes().chunks(2) {
        let hi = char::from(pair[0]).to_digit(16)?;
        let lo = char::from(pair[1]).to_digit(16)?;
        out.push(u8::try_from(hi << 4 | lo).ok()?);
    }
    Some(out)
}

fn err(kind: ErrorKind, message: impl Into<String>) -> RemoteError {
    RemoteError {
        kind,
        message: message.into(),
    }
}

/// The setting naming the vault an upgrade copies into the vault `new_id` (written when the
/// upgrade makes it): only that upgrade may resume into it.
fn upgraded_from_key(new_id: &str) -> String {
    format!("vault.upgraded_from.{new_id}")
}

fn store_err(e: wasl_store::StoreError) -> RemoteError {
    crate::daemon::store_err(e)
}

fn drive_err(e: wasl_drive::DriveError) -> RemoteError {
    crate::daemon::drive_err(e)
}

fn unusable(e: CryptoError) -> RemoteError {
    match e {
        CryptoError::Io(e) => err(ErrorKind::Other, e.to_string()),
        e => err(
            ErrorKind::Invalid,
            format!("This vault's vault.json can't be used: {e}"),
        ),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Why a vault's keys are not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoKeys {
    /// No vault with this id on this computer.
    Unknown,
    /// Known, not unlocked here.
    Locked,
    /// The keychain holds something that is not this vault's keys.
    Damaged(String),
}

/// The vaults of this computer.
pub struct Vaults {
    store: SharedStore,
    keychain: Arc<dyn Secrets>,
    /// `<data>/tmp/vaults`: ciphertext and manifests in passing.
    tmp: PathBuf,
    /// The cost of new vaults.
    kdf: KdfParams,
    /// One key derivation at a time: each takes 64 MiB and a second or more.
    deriving: tokio::sync::Mutex<()>,
    /// The reads of every vault view, cut off when their vault is locked.
    gates: Mutex<Vec<Weak<Gate>>>,
    /// Whether each vault's keys are in the keychain, as last read or written here, so showing
    /// a vault's state does not read its secret every time (security review S4-5). The keychain
    /// has no way to ask whether an entry exists without reading it, so each vault's entry is read
    /// at most once per start for this; unlocking, locking and using the keys keep it current.
    unlocked: Mutex<HashMap<String, bool>>,
}

/// A vault view's reads. Locking the vault cuts it off at once: the keys it holds are dropped
/// and every later read fails with [`VAULT_LOCKED`], whoever still holds the view's hydrator (a
/// fetch Windows asked for just before, a FUSE request in flight).
struct Gate {
    vault_id: String,
    source: Mutex<Option<Arc<dyn Source>>>,
}

impl Gate {
    fn get(&self) -> Option<Arc<dyn Source>> {
        self.source.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn close(&self) {
        self.source.lock().unwrap_or_else(|p| p.into_inner()).take();
    }
}

#[async_trait]
impl Source for Gate {
    async fn read(&self, file: &FileRef, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        match self.get() {
            Some(source) => source.read(file, offset, len).await,
            None => Err(VfsError::Unavailable(VAULT_LOCKED.into())),
        }
    }

    fn natural_range(&self, file: &FileRef, offset: u64, len: u64) -> (u64, u64) {
        match self.get() {
            Some(source) => source.natural_range(file, offset, len),
            None => (offset, len),
        }
    }
}

impl Vaults {
    pub fn new(store: SharedStore, keychain: Arc<dyn Secrets>, tmp: impl Into<PathBuf>) -> Vaults {
        Vaults {
            store,
            keychain,
            tmp: tmp.into(),
            kdf: KdfParams::DEFAULT,
            deriving: tokio::sync::Mutex::new(()),
            gates: Mutex::new(Vec::new()),
            unlocked: Mutex::new(HashMap::new()),
        }
    }

    fn set_unlocked(&self, id: &str, unlocked: bool) {
        self.unlocked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.to_owned(), unlocked);
    }

    /// Whether the vault's keys are kept here, without reading the secret when it is known.
    fn is_unlocked(&self, id: &str) -> bool {
        if let Some(known) = self.unlocked.lock().unwrap_or_else(|p| p.into_inner()).get(id) {
            return *known;
        }
        // First time since the start: read once, wipe at once, remember the answer.
        let present = self.keychain.get(id).map(Zeroizing::new).is_some();
        self.set_unlocked(id, present);
        present
    }

    /// The Argon2id cost of vaults made from now on (tests use [`KdfParams::MIN`]).
    pub fn with_kdf(mut self, kdf: KdfParams) -> Vaults {
        self.kdf = kdf;
        self
    }

    fn record(&self, id: &str) -> Result<VaultRecord, RemoteError> {
        self.store
            .lock()
            .vault(id)
            .map_err(store_err)?
            .ok_or_else(|| err(ErrorKind::NotFound, format!("No vault {id} on this computer")))
    }

    fn info_of(&self, r: VaultRecord) -> VaultInfo {
        let unlocked = self.is_unlocked(&r.id);
        let format = match Manifest::parse(&r.manifest_json) {
            Ok(Some(Manifest::V1(_))) => "1".to_owned(),
            Ok(Some(Manifest::V2(m))) => m.format().label().to_owned(),
            _ => String::new(),
        };
        VaultInfo {
            id: r.id,
            name: r.name,
            version: r.format_version,
            format,
            unlocked,
            account_id: r.account_id,
            drive_folder_id: r.drive_folder_id,
            drive_id: r.drive_id,
            remote_path: r.remote_path,
            created_at: r.created_at,
        }
    }

    /// Every vault known here, with whether it is unlocked.
    pub fn list(&self) -> Result<Vec<VaultInfo>, RemoteError> {
        let records = self.store.lock().vaults().map_err(store_err)?;
        Ok(records.into_iter().map(|r| self.info_of(r)).collect())
    }

    pub fn info(&self, id: &str) -> Result<VaultInfo, RemoteError> {
        Ok(self.info_of(self.record(id)?))
    }

    /// The vault's keys from the keychain, checked against its key check.
    pub fn keys(&self, id: &str) -> Result<Keys, NoKeys> {
        let record = self
            .store
            .lock()
            .vault(id)
            .ok()
            .flatten()
            .ok_or(NoKeys::Unknown)?;
        let stored = self.keychain.get(id).map(Zeroizing::new);
        self.set_unlocked(id, stored.is_some());
        let stored = stored.ok_or(NoKeys::Locked)?;
        let damaged = |why: &str| NoKeys::Damaged(why.to_owned());
        let (version, hex) = stored.split_once(':').ok_or_else(|| damaged("not a vault key"))?;
        let bytes = decode_hex(hex).ok_or_else(|| damaged("not a vault key"))?;
        let manifest = Manifest::parse(&record.manifest_json)
            .ok()
            .flatten()
            .ok_or_else(|| damaged("its vault.json can't be read"))?;
        match (manifest, version) {
            (Manifest::V1(manifest), "1") => {
                let keys =
                    VaultKeys::from_master(&bytes).map_err(|_| damaged("a v1 vault keeps 64 bytes"))?;
                let check = manifest.key_check_bytes().unwrap_or_default();
                if !wasl_crypto::kdf::key_check_matches(&keys, &check) {
                    return Err(damaged("they are another vault's"));
                }
                Ok(Keys::V1 {
                    keys: Arc::new(keys),
                    manifest,
                })
            }
            (Manifest::V2(manifest), "2") => {
                let ikm: Zeroizing<[u8; v2keys::IKM_BYTES]> = Zeroizing::new(
                    bytes
                        .as_slice()
                        .try_into()
                        .map_err(|_| damaged("a v2 vault keeps 32 bytes"))?,
                );
                let keys = VaultKeysV2::from_ikm(&ikm);
                let check = manifest.key_check_bytes().unwrap_or_default();
                if !keys.key_check_matches(&check) {
                    return Err(damaged("they are another vault's"));
                }
                Ok(Keys::V2 {
                    keys: Arc::new(keys),
                    manifest,
                })
            }
            _ => Err(damaged("they are of another format")),
        }
    }

    /// Checks a new vault's name and passphrase before anything is made on Drive.
    pub fn check_new(name: &str, passphrase: &Passphrase) -> Result<(), RemoteError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(err(ErrorKind::Invalid, "A vault needs a name"));
        }
        if name.contains(['/', '\\']) || name == "." || name == ".." {
            return Err(err(ErrorKind::Invalid, "A vault's name can't contain / or \\"));
        }
        if passphrase.expose().chars().count() < MIN_PASSPHRASE_CHARS {
            return Err(err(
                ErrorKind::Invalid,
                format!("A vault's passphrase needs at least {MIN_PASSPHRASE_CHARS} characters"),
            ));
        }
        Ok(())
    }

    /// Makes a new vault (format v2) in `folder`, which must be new or empty: uploads its
    /// `vault.json`, records it and keeps its keys here.
    pub async fn create(
        &self,
        drive: &dyn Remote,
        folder: VaultFolder,
        name: &str,
        passphrase: Passphrase,
    ) -> Result<VaultInfo, RemoteError> {
        Self::check_new(name, &passphrase)?;
        let name = name.trim();
        let listing = drive
            .list_top_level(&folder.folder_id, folder.drive_id.as_deref())
            .await
            .map_err(drive_err)?;
        if listing.entries.iter().any(|e| !e.is_dir && e.key == "vault.json") {
            return Err(err(
                ErrorKind::Invalid,
                format!("{} already holds a vault. Unlock it instead.", folder.remote_path),
            ));
        }
        if !listing.entries.is_empty() || !listing.duplicates.is_empty() || !listing.shortcuts.is_empty() {
            return Err(err(
                ErrorKind::Invalid,
                format!(
                    "{} isn't empty. A new vault needs a new or empty Drive folder.",
                    folder.remote_path
                ),
            ));
        }
        let kdf = self.kdf;
        let (salt, ikm) = {
            let _one = self.deriving.lock().await;
            tokio::task::spawn_blocking(move || {
                let salt = v2keys::new_salt()?;
                let ikm = v2keys::argon2id(passphrase.expose(), &salt, kdf)?;
                Ok::<_, CryptoError>((salt, ikm))
            })
            .await
            .map_err(|e| err(ErrorKind::Other, e.to_string()))?
            .map_err(unusable)?
        };
        let keys = VaultKeysV2::from_ikm(&ikm);
        let manifest = VaultManifestV2::create(name, &salt, kdf, &keys.key_check(), now_ms());
        drop(keys);
        wasl_engine::ensure_manifest_v2(
            drive,
            &folder.folder_id,
            folder.drive_id.as_deref(),
            &manifest,
            &self.tmp,
        )
        .await
        .map_err(|e| match e {
            wasl_drive::DriveError::Integrity(m) => err(ErrorKind::Invalid, m),
            e => drive_err(e),
        })?;
        self.keep(Manifest::V2(manifest), &folder, Material::V2(ikm))
    }

    /// Records the vault (keeping the Drive folder it was first seen in) and its keys.
    fn keep(
        &self,
        manifest: Manifest,
        folder: &VaultFolder,
        material: Material,
    ) -> Result<VaultInfo, RemoteError> {
        let record = self.remember(&manifest, folder)?;
        self.keychain.set(&record.id, &material.encode()).map_err(|e| {
            err(
                ErrorKind::Other,
                format!("{} can't be kept unlocked: {e}", record.name),
            )
        })?;
        self.set_unlocked(&record.id, true);
        Ok(self.info_of(record))
    }

    /// The vault's record, made when this computer has not seen the vault before.
    fn remember(&self, manifest: &Manifest, folder: &VaultFolder) -> Result<VaultRecord, RemoteError> {
        let id = manifest.id();
        let s = self.store.lock();
        if let Some(mut known) = s.vault(&id).map_err(store_err)? {
            if known.drive_folder_id.is_none() {
                known.account_id = Some(folder.account_id.clone());
                known.drive_folder_id = Some(folder.folder_id.clone());
                known.drive_id = folder.drive_id.clone();
                known.remote_path = folder.remote_path.clone();
                s.put_vault(&known).map_err(store_err)?;
            }
            return Ok(known);
        }
        let record = VaultRecord {
            id,
            name: manifest.name().to_owned(),
            format_version: manifest.version(),
            kdf_params_json: manifest.kdf_json(),
            key_check: manifest.key_check(),
            manifest_json: manifest.json(),
            account_id: Some(folder.account_id.clone()),
            drive_folder_id: Some(folder.folder_id.clone()),
            drive_id: folder.drive_id.clone(),
            remote_path: folder.remote_path.clone(),
            created_at: manifest.created_ms(),
        };
        s.put_vault(&record).map_err(store_err)?;
        Ok(record)
    }

    /// Derives the keys for `passphrase` off the async threads; `None` for a wrong passphrase.
    async fn derive(
        &self,
        manifest: Manifest,
        passphrase: Passphrase,
    ) -> Result<Option<Material>, RemoteError> {
        if passphrase.is_empty() {
            return Err(err(ErrorKind::Invalid, "Type the vault's passphrase"));
        }
        let _one = self.deriving.lock().await;
        tokio::task::spawn_blocking(move || match manifest {
            Manifest::V1(m) => Ok(m.verify_passphrase(passphrase.expose())?.map(Material::V1)),
            Manifest::V2(m) => {
                let ikm = v2keys::argon2id(passphrase.expose(), &m.salt_bytes()?, m.params()?)?;
                let right = VaultKeysV2::from_ikm(&ikm).key_check_matches(&m.key_check_bytes()?);
                Ok(right.then_some(Material::V2(ikm)))
            }
        })
        .await
        .map_err(|e| err(ErrorKind::Other, e.to_string()))?
        .map_err(unusable)
    }

    /// Unlocks a vault this computer knows.
    pub async fn unlock(&self, id: &str, passphrase: Passphrase) -> Result<VaultInfo, RemoteError> {
        let record = self.record(id)?;
        let manifest = Manifest::parse(&record.manifest_json)
            .map_err(unusable)?
            .ok_or_else(|| err(ErrorKind::Invalid, "This vault's vault.json can't be read"))?;
        let material = self
            .derive(manifest, passphrase)
            .await?
            .ok_or_else(|| err(ErrorKind::Invalid, WRONG_PASSPHRASE))?;
        self.keychain.set(&record.id, &material.encode()).map_err(|e| {
            err(
                ErrorKind::Other,
                format!("{} can't be kept unlocked: {e}", record.name),
            )
        })?;
        self.set_unlocked(&record.id, true);
        Ok(self.info_of(record))
    }

    /// Unlocks the vault a Drive folder holds (v1 or v2), recording it when it is new here.
    pub async fn unlock_folder(
        &self,
        drive: &dyn Remote,
        folder: VaultFolder,
        passphrase: Passphrase,
    ) -> Result<VaultInfo, RemoteError> {
        let manifest = self.read_manifest(drive, &folder).await?.ok_or_else(|| {
            err(
                ErrorKind::Invalid,
                format!("{} isn't a vault: it has no vault.json.", folder.remote_path),
            )
        })?;
        let material = self
            .derive(manifest.clone(), passphrase)
            .await?
            .ok_or_else(|| err(ErrorKind::Invalid, WRONG_PASSPHRASE))?;
        self.keep(manifest, &folder, material)
    }

    /// The vault a Drive folder holds, recorded here (locked until it is unlocked) when it is new;
    /// `None` for a folder that is not a vault.
    pub async fn adopt_folder(
        &self,
        drive: &dyn Remote,
        folder: &VaultFolder,
    ) -> Result<Option<VaultRecord>, RemoteError> {
        match self.read_manifest(drive, folder).await? {
            Some(m) => self.remember(&m, folder).map(Some),
            None => Ok(None),
        }
    }

    /// The folder's `vault.json`, when it has one of Wasl's.
    async fn read_manifest(
        &self,
        drive: &dyn Remote,
        folder: &VaultFolder,
    ) -> Result<Option<Manifest>, RemoteError> {
        let listing = drive
            .list_top_level(&folder.folder_id, folder.drive_id.as_deref())
            .await
            .map_err(drive_err)?;
        let Some(entry) = listing
            .entries
            .iter()
            .find(|e| e.key == "vault.json" && !e.is_dir && e.google_mime.is_none())
        else {
            return Ok(None);
        };
        root_vault(drive, entry, &self.tmp).await.map_err(|e| match e {
            wasl_drive::DriveError::Integrity(m) => {
                err(ErrorKind::Invalid, format!("The {m} ({}).", folder.remote_path))
            }
            e => drive_err(e),
        })
    }

    /// Links a pair to the vault its Drive folder holds before it is recorded, so it can never
    /// sync a vault's folder as plain files (plain files would go up unencrypted, ciphertext would
    /// come down as files). A pair that names a vault must name one known here. When the folder
    /// cannot be read now, the pair is refused: whether it is a vault is not known.
    pub async fn link_pair(&self, drive: &dyn Remote, pair: &mut Pair) -> Result<(), RemoteError> {
        if let Some(id) = &pair.vault_id {
            self.info(id)?;
            return Ok(());
        }
        let Some(folder) = folder_of_pair(pair) else {
            return Ok(());
        };
        let unknown = |why: String| {
            format!(
                "Wasl couldn't check whether {} on Drive is an encrypted vault ({why}). Try again when Drive can be reached.",
                folder.remote_path
            )
        };
        match tokio::time::timeout(LINK_PAIR_WITHIN, self.adopt_folder(drive, &folder)).await {
            Ok(Ok(Some(vault))) => {
                pair.vault_id = Some(vault.id);
                Ok(())
            }
            Ok(Ok(None)) => Ok(()),
            Ok(Err(e)) if matches!(e.kind, ErrorKind::Invalid | ErrorKind::NotFound) => Err(e),
            Ok(Err(e)) => Err(err(e.kind, unknown(e.message))),
            Err(_) => Err(err(
                ErrorKind::Network,
                unknown("Drive didn't answer in time".into()),
            )),
        }
    }

    /// Moves a vault to the newest format by copying it into a new vault in `folder` (security
    /// review S4-2: v1's 64-bit nonces and PBKDF2, 2.0's public chunking and exact sizes). The old
    /// vault must be unlocked here; `passphrase` must be its passphrase (checked against it), and
    /// becomes the new vault's, with a new salt. `folder` must be new or empty, or hold the vault
    /// an earlier, interrupted upgrade of this same vault made on this computer (recorded when it
    /// was made): then the copy continues (files already there are not copied again). Any other
    /// vault there is refused, even one the passphrase opens.
    ///
    /// Nothing of the old vault changes: its folder, its pairs and its views stay as they are, so
    /// they can be compared and moved one at a time (docs/ENCRYPTION.md §11.12 lists the steps).
    /// Pairs are not moved here: a pair's history (its baseline) belongs to its Drive folder, and
    /// pointing it at another folder would make its next run treat every file as new on both sides.
    pub async fn upgrade(
        &self,
        drive: Arc<dyn Remote>,
        id: &str,
        folder: VaultFolder,
        passphrase: Passphrase,
    ) -> Result<(VaultInfo, CopyReport), RemoteError> {
        let record = self.record(id)?;
        let old_folder = record.drive_folder_id.clone().ok_or_else(|| {
            err(
                ErrorKind::Invalid,
                format!(
                    "Wasl doesn't know {}'s Drive folder. Unlock it from its folder first.",
                    record.name
                ),
            )
        })?;
        if old_folder == folder.folder_id {
            return Err(err(
                ErrorKind::Invalid,
                "The upgraded vault needs a new Drive folder, not the vault's own.",
            ));
        }
        let old = self.keys(id).map_err(|_| {
            err(
                ErrorKind::Locked,
                format!("Unlock {} on this computer before upgrading it.", record.name),
            )
        })?;
        if matches!(&old, Keys::V2 { manifest, .. } if manifest.format() == Format::LATEST) {
            return Err(err(
                ErrorKind::Invalid,
                format!(
                    "{} is already in the newest format ({}).",
                    record.name,
                    Format::LATEST.label()
                ),
            ));
        }
        // The same passphrase as the old vault's: an upgrade does not change what people type.
        let old_manifest = Manifest::parse(&record.manifest_json)
            .map_err(unusable)?
            .ok_or_else(|| err(ErrorKind::Invalid, "This vault's vault.json can't be read"))?;
        if self.derive(old_manifest, passphrase.clone()).await?.is_none() {
            return Err(err(ErrorKind::Invalid, WRONG_PASSPHRASE));
        }

        // The new vault: made now, or the one an interrupted upgrade of this vault made here.
        // Any other vault the passphrase opens (one made by hand with the same passphrase, or an
        // upgrade of another vault) is never written into: the copy would mix two vaults.
        let resumed = self.read_manifest(drive.as_ref(), &folder).await?;
        let new = match resumed {
            Some(Manifest::V2(m)) if m.format() == Format::LATEST => {
                let made_by = self
                    .store
                    .lock()
                    .setting(&upgraded_from_key(&Manifest::V2(m).id()))
                    .map_err(store_err)?;
                if made_by.as_deref() != Some(id) {
                    return Err(err(
                        ErrorKind::Invalid,
                        format!(
                            "{} already holds a vault that isn't an upgrade of {} started on this computer. Choose a new or empty folder.",
                            folder.remote_path, record.name
                        ),
                    ));
                }
                self.unlock_folder(drive.as_ref(), folder.clone(), passphrase)
                    .await?
            }
            Some(_) => {
                return Err(err(
                    ErrorKind::Invalid,
                    format!(
                        "{} already holds another kind of vault. Choose a new or empty folder.",
                        folder.remote_path
                    ),
                ));
            }
            None => {
                let made = self
                    .create(drive.as_ref(), folder.clone(), &record.name, passphrase)
                    .await?;
                // Recorded before anything is copied, so only this upgrade resumes into it.
                self.store
                    .lock()
                    .set_setting(&upgraded_from_key(&made.id), id)
                    .map_err(store_err)?;
                made
            }
        };
        let Ok(Keys::V2 {
            keys: new_keys,
            manifest: new_manifest,
        }) = self.keys(&new.id)
        else {
            return Err(err(
                ErrorKind::Other,
                "The upgraded vault's keys can't be read back",
            ));
        };

        let tmp = self.tmp.join("upgrade").join(&new.id);
        let from: Arc<dyn Remote> = match old {
            Keys::V1 { keys, manifest } => {
                Arc::new(VaultRemote::new(drive.clone(), keys, tmp.join("old")).with_manifest(manifest))
            }
            Keys::V2 { keys, manifest } => {
                Arc::new(VaultV2Remote::new(drive.clone(), keys, tmp.join("old")).with_manifest(manifest))
            }
        };
        let to = VaultV2Remote::new(drive, new_keys, tmp.join("new")).with_manifest(new_manifest);
        let report = copy_vault(
            from.as_ref(),
            &old_folder,
            record.drive_id.as_deref(),
            &to,
            &folder.folder_id,
            folder.drive_id.as_deref(),
            &tmp.join("plain"),
        )
        .await
        .map_err(|e| match e {
            wasl_drive::DriveError::Integrity(m) => err(ErrorKind::Invalid, m),
            e => drive_err(e),
        })?;
        let _ = std::fs::remove_dir_all(&tmp);
        Ok((new, report))
    }

    /// Forgets the vault's keys on this computer, and cuts off every view reading through them.
    pub fn lock(&self, id: &str) -> Result<(), RemoteError> {
        self.record(id)?;
        self.keychain.remove(id);
        self.set_unlocked(id, false);
        let mut gates = self.gates.lock().unwrap_or_else(|p| p.into_inner());
        gates.retain(|g| match g.upgrade() {
            Some(gate) if gate.vault_id == id => {
                gate.close();
                false
            }
            Some(_) => true,
            None => false,
        });
        Ok(())
    }

    /// `source` behind a gate that [`Self::lock`] closes.
    fn gated(&self, vault_id: &str, source: Arc<dyn Source>) -> Arc<dyn Source> {
        let gate = Arc::new(Gate {
            vault_id: vault_id.to_owned(),
            source: Mutex::new(Some(source)),
        });
        let mut gates = self.gates.lock().unwrap_or_else(|p| p.into_inner());
        gates.retain(|g| g.strong_count() > 0);
        gates.push(Arc::downgrade(&gate));
        gate
    }

    /// What a pair syncs through: the account's Drive, or for a vault pair the vault's remote over
    /// it (v1 or v2, as the phone builds them). A locked vault stops the pair with
    /// [`EngineError::Locked`].
    pub fn pair_remote(&self, drive: Arc<dyn Remote>, pair: &Pair) -> wasl_engine::Result<Arc<dyn Remote>> {
        let tmp = self.tmp.join("pair").join(&pair.id);
        let Some(vault_id) = &pair.vault_id else {
            return Ok(Arc::new(PlainGuard::new(drive, tmp, pair.clone())));
        };
        match self.keys(vault_id) {
            Ok(Keys::V1 { keys, manifest }) => Ok(Arc::new(
                VaultRemote::new(drive, keys, tmp).with_manifest(manifest),
            )),
            Ok(Keys::V2 { keys, manifest }) => Ok(Arc::new(
                VaultV2Remote::new(drive, keys, tmp)
                    .with_manifest(manifest)
                    .with_store(self.store.clone(), pair.id.clone()),
            )),
            Err(NoKeys::Locked) => {
                let name = self.record(vault_id).map(|r| r.name).unwrap_or_default();
                Err(EngineError::Locked(format!(
                    "The vault {name} is locked on this computer. Unlock it to sync this pair."
                )))
            }
            Err(NoKeys::Unknown) => Err(EngineError::Locked(
                "This pair's vault isn't on this computer yet. Unlock its Drive folder to sync this pair."
                    .into(),
            )),
            Err(NoKeys::Damaged(why)) => Err(EngineError::Locked(format!(
                "The vault's keys on this computer can't be used ({why}). Lock the vault and unlock it again."
            ))),
        }
    }

    /// What a view reads through: plain Drive, or a vault's decrypted listing (names and
    /// plaintext sizes) with its bytes read a chunk at a time.
    pub fn view_remote(&self, drive: Arc<dyn Remote>, view: &View) -> Result<ViewRemote, RemoteError> {
        let Some(vault_id) = &view.vault_id else {
            return Ok(ViewRemote::plain(drive));
        };
        let tmp = self.tmp.join("view").join(&view.id);
        match self.keys(vault_id) {
            Ok(Keys::V1 { keys, .. }) => {
                let cipher = Arc::new(VaultCipher::new(&keys));
                Ok(ViewRemote {
                    remote: Arc::new(VaultRemote::new(drive.clone(), keys, tmp)),
                    source: self.gated(vault_id, Arc::new(VaultV1Source::new(drive, cipher))),
                })
            }
            Ok(Keys::V2 { keys, .. }) => {
                // Views are not pairs: the highest manifest versions are kept in memory only.
                let remote = Arc::new(VaultV2Remote::new(drive, keys, tmp));
                Ok(ViewRemote {
                    remote: remote.clone(),
                    source: self.gated(vault_id, remote),
                })
            }
            Err(NoKeys::Locked | NoKeys::Unknown) => Err(err(ErrorKind::Locked, VAULT_LOCKED)),
            Err(NoKeys::Damaged(why)) => Err(err(
                ErrorKind::Locked,
                format!(
                    "The vault's keys on this computer can't be used ({why}). Lock the vault and unlock it again."
                ),
            )),
        }
    }
}

/// The account's Drive, by account id.
pub type DriveOf = Arc<dyn Fn(&str) -> Arc<dyn Remote> + Send + Sync>;

/// The views of this computer read through the account's Drive, and a vault folder through its
/// vault.
pub struct VaultViews {
    pub vaults: Arc<Vaults>,
    /// The account's Drive.
    pub drive: DriveOf,
}

#[async_trait]
impl ViewRemotes for VaultViews {
    fn open(&self, view: &View) -> Result<ViewRemote, RemoteError> {
        self.vaults.view_remote((self.drive)(&view.account_id), view)
    }

    async fn vault_of(&self, view: &View) -> Result<Option<String>, RemoteError> {
        let folder = VaultFolder {
            account_id: view.account_id.clone(),
            folder_id: view.drive_folder_id.clone(),
            drive_id: view.drive_id.clone(),
            remote_path: view.remote_path.clone(),
        };
        let drive = (self.drive)(&view.account_id);
        Ok(self
            .vaults
            .adopt_folder(drive.as_ref(), &folder)
            .await?
            .map(|r| r.id))
    }
}

/// Plain reads of a plain view.
impl ViewRemote {
    pub fn plain(remote: Arc<dyn Remote>) -> ViewRemote {
        ViewRemote {
            source: Arc::new(RemoteSource(remote.clone())),
            remote,
        }
    }
}

/// What a `vault.json` at a folder's root is: a Wasl vault's, or definitely someone else's file.
/// Unreadable, oversized or unparsable is an error (fail closed).
async fn root_vault(
    drive: &dyn Remote,
    entry: &wasl_core::RemoteEntry,
    tmp: &Path,
) -> Result<Option<Manifest>, wasl_drive::DriveError> {
    let text = manifest_text(drive, entry, tmp).await?;
    Manifest::parse(&text).map_err(|e| match e {
        CryptoError::Integrity(m) if m == DAMAGED_MANIFEST => wasl_drive::DriveError::Integrity(m),
        e => wasl_drive::DriveError::Integrity(format!("{DAMAGED_MANIFEST} ({e})")),
    })
}

pub use wasl_engine::PlainGuard;

/// The vault's folder on Drive as a [`VaultFolder`], for building one from a pair.
pub fn folder_of_pair(pair: &Pair) -> Option<VaultFolder> {
    Some(VaultFolder {
        account_id: pair.account_id.clone()?,
        folder_id: pair.drive_folder_id.clone()?,
        drive_id: pair.drive_id.clone(),
        remote_path: pair.remote_path.clone(),
    })
}

/// Where vault temporaries go under the data folder.
pub fn tmp_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tmp").join("vaults")
}
