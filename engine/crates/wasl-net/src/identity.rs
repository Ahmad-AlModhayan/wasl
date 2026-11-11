//! Device identity (docs/DEVICE_SYNC.md §2): an Ed25519 key per install and the IDs derived
//! from its public key.
//!
//! - [`DeviceKey`]: the signing key; its secret bytes are zeroized on drop.
//! - [`DeviceId`]: the first 16 bytes of BLAKE3(public key). Written as
//!   `WASL-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX`: 26 symbols carrying the 128 bits (the last two
//!   padding bits must be zero) and two check symbols, in Wasl's base32 alphabet
//!   (`0-9 A-Z` without `I`, `L`, `O`, `U`). Check symbol 1 is Luhn mod 32 over the 26 data symbols, so every
//!   single mistyped symbol and most swaps of neighbours are caught; check symbol 2 is 5 bits of
//!   BLAKE3 over the ID, so a random corruption passes with probability ~1/1024 at most.
//! - [`DeviceId::short_id`]: the first 8 bytes as a big-endian `u64` (version vectors).
//! - [`KeyStore`]: where the 32-byte seed lives. The OS keychain implementation belongs to the
//!   daemon; [`MemoryKeyStore`] is for tests.

use std::fmt;
use std::str::FromStr;
use std::sync::Mutex;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use zeroize::Zeroizing;

use crate::base32;

/// Length of a device ID in bytes.
pub const DEVICE_ID_LEN: usize = 16;
const ID_DATA_SYMBOLS: usize = 26;
const ID_PREFIX: &str = "WASL";

/// Errors from this module.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdentityError {
    #[error("not a Wasl device ID")]
    BadDeviceId,
    #[error("not a valid Ed25519 public key")]
    BadPublicKey,
    #[error("the system random generator failed")]
    Rng,
    #[error("key store: {0}")]
    KeyStore(String),
}

/// A device's Ed25519 signing key. The secret is zeroized when this is dropped.
pub struct DeviceKey {
    signing: SigningKey,
}

impl DeviceKey {
    /// A new random key.
    pub fn generate() -> Result<Self, IdentityError> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut()).map_err(|_| IdentityError::Rng)?;
        Ok(Self::from_seed(&seed))
    }

    /// The key for a stored 32-byte seed.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(seed),
        }
    }

    /// Loads the key from `store`, or creates one and saves it there.
    pub fn load_or_create(store: &dyn KeyStore) -> Result<Self, IdentityError> {
        if let Some(seed) = store.load()? {
            return Ok(Self::from_seed(&seed));
        }
        let key = Self::generate()?;
        store.save(&key.seed())?;
        Ok(key)
    }

    /// The secret seed, for the key store only.
    pub fn seed(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.signing.to_bytes())
    }

    /// The public key bytes.
    pub fn public(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    /// This device's ID.
    pub fn id(&self) -> DeviceId {
        DeviceId::from_public_key(&self.public())
    }

    /// An Ed25519 signature over `msg`.
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.signing.sign(msg).to_bytes()
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceKey")
            .field("id", &self.id())
            .finish_non_exhaustive()
    }
}

/// Parses a public key, refusing invalid points and small-order ("weak") keys.
pub fn parse_public_key(pk: &[u8; 32]) -> Result<VerifyingKey, IdentityError> {
    let key = VerifyingKey::from_bytes(pk).map_err(|_| IdentityError::BadPublicKey)?;
    if key.is_weak() {
        return Err(IdentityError::BadPublicKey);
    }
    Ok(key)
}

/// Verifies an Ed25519 signature strictly (no malleable or small-order forms).
pub fn verify_signature(pk: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
    let Ok(key) = parse_public_key(pk) else {
        return false;
    };
    let sig = Signature::from_bytes(sig);
    key.verify_strict(msg, &sig).is_ok()
}

/// A device ID: the first 16 bytes of BLAKE3(public key).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId([u8; DEVICE_ID_LEN]);

impl DeviceId {
    /// The ID of a public key.
    pub fn from_public_key(pk: &[u8; 32]) -> Self {
        let hash = blake3::hash(pk);
        let mut id = [0u8; DEVICE_ID_LEN];
        id.copy_from_slice(&hash.as_bytes()[..DEVICE_ID_LEN]);
        Self(id)
    }

    /// An ID from its raw bytes.
    pub fn from_bytes(bytes: [u8; DEVICE_ID_LEN]) -> Self {
        Self(bytes)
    }

    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8; DEVICE_ID_LEN] {
        &self.0
    }

    /// The short ID used in version vectors: the first 8 bytes, big-endian.
    pub fn short_id(&self) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.0[..8]);
        u64::from_be_bytes(b)
    }

    /// Parses `WASL-XXXX-…`: any case, `-` and spaces optional, look-alikes folded, both check
    /// symbols and the zero padding verified.
    pub fn parse(text: &str) -> Result<Self, IdentityError> {
        let trimmed = text.trim();
        let rest = trimmed
            .get(..ID_PREFIX.len())
            .filter(|p| p.eq_ignore_ascii_case(ID_PREFIX))
            .and_then(|_| trimmed.get(ID_PREFIX.len()..))
            .ok_or(IdentityError::BadDeviceId)?;
        let values = base32::read_typed(rest).ok_or(IdentityError::BadDeviceId)?;
        if values.len() != ID_DATA_SYMBOLS + 2 {
            return Err(IdentityError::BadDeviceId);
        }
        let (data, check) = values.split_at(ID_DATA_SYMBOLS);
        let bytes = base32::from_values(data, DEVICE_ID_LEN).ok_or(IdentityError::BadDeviceId)?;
        let mut id = [0u8; DEVICE_ID_LEN];
        id.copy_from_slice(&bytes);
        let id = Self(id);
        if check != id.check_symbols() {
            return Err(IdentityError::BadDeviceId);
        }
        Ok(id)
    }

    fn check_symbols(&self) -> [u8; 2] {
        let data = base32::to_values(&self.0);
        let mut ctx = blake3::Hasher::new();
        ctx.update(b"wasl-id-check");
        ctx.update(&self.0);
        [luhn32(&data), ctx.finalize().as_bytes()[0] >> 3]
    }
}

/// Luhn mod 32 check value: doubling every second value from the right, folded back into
/// 0..32 (a permutation of 0..32, so any single substitution changes the sum).
fn luhn32(values: &[u8]) -> u8 {
    let mut sum = 0u32;
    for (i, &v) in values.iter().rev().enumerate() {
        let mut addend = u32::from(v);
        if i % 2 == 0 {
            addend *= 2;
            addend = addend / 32 + addend % 32;
        }
        sum += addend;
    }
    ((32 - sum % 32) % 32) as u8
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut values = base32::to_values(&self.0);
        values.extend_from_slice(&self.check_symbols());
        let text = base32::write(&values);
        f.write_str(ID_PREFIX)?;
        for group in text.as_bytes().chunks(4) {
            f.write_str("-")?;
            f.write_str(std::str::from_utf8(group).map_err(|_| fmt::Error)?)?;
        }
        Ok(())
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for DeviceId {
    type Err = IdentityError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl serde::Serialize for DeviceId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for DeviceId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Where a device keeps its 32-byte key seed. The daemon wires the OS keychain; this crate only
/// ships [`MemoryKeyStore`].
pub trait KeyStore {
    /// The stored seed, if any.
    fn load(&self) -> Result<Option<Zeroizing<[u8; 32]>>, IdentityError>;
    /// Stores the seed, replacing any previous one.
    fn save(&self, seed: &[u8; 32]) -> Result<(), IdentityError>;
}

/// An in-memory [`KeyStore`] for tests.
#[derive(Default)]
pub struct MemoryKeyStore {
    seed: Mutex<Option<Zeroizing<[u8; 32]>>>,
}

impl KeyStore for MemoryKeyStore {
    fn load(&self) -> Result<Option<Zeroizing<[u8; 32]>>, IdentityError> {
        let guard = self
            .seed
            .lock()
            .map_err(|_| IdentityError::KeyStore("poisoned".into()))?;
        Ok(guard.clone())
    }

    fn save(&self, seed: &[u8; 32]) -> Result<(), IdentityError> {
        let mut guard = self
            .seed
            .lock()
            .map_err(|_| IdentityError::KeyStore("poisoned".into()))?;
        *guard = Some(Zeroizing::new(*seed));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> DeviceKey {
        DeviceKey::from_seed(&[n; 32])
    }

    #[test]
    fn id_is_deterministic_from_key() {
        let a = key(7);
        let b = key(7);
        assert_eq!(a.public(), b.public());
        assert_eq!(a.id(), b.id());
        assert_ne!(a.id(), key(8).id());
        let hash = blake3::hash(&a.public());
        assert_eq!(a.id().as_bytes()[..], hash.as_bytes()[..16]);
        assert_eq!(
            a.id().short_id(),
            u64::from_be_bytes(hash.as_bytes()[..8].try_into().unwrap())
        );
    }

    #[test]
    fn display_shape_and_round_trip() {
        for n in 0..50u8 {
            let id = key(n).id();
            let text = id.to_string();
            assert_eq!(text.len(), "WASL-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX".len());
            assert!(text.starts_with("WASL-"));
            assert_eq!(DeviceId::parse(&text), Ok(id));
            // Typed forms: lower case, no dashes, spaces.
            assert_eq!(DeviceId::parse(&text.to_lowercase()), Ok(id));
            assert_eq!(DeviceId::parse(&text.replace('-', "")), Ok(id));
            assert_eq!(DeviceId::parse(&text.replace('-', " ")), Ok(id));
        }
    }

    #[test]
    fn look_alikes_read_as_their_symbol() {
        let id = DeviceId::from_bytes([0; 16]);
        let text = id.to_string();
        let typed = format!("WASL{}", text[4..].replace('0', "O").replace('1', "l"));
        assert_eq!(DeviceId::parse(&typed), Ok(id));
    }

    #[test]
    fn every_single_symbol_corruption_is_rejected() {
        for n in 0..8u8 {
            let id = key(n).id();
            let text = id.to_string();
            for (pos, c) in text.char_indices().skip(5) {
                if c == '-' {
                    continue;
                }
                for &sym in base32::ALPHABET.iter() {
                    if sym as char == c {
                        continue;
                    }
                    let mut bad = text.clone().into_bytes();
                    bad[pos] = sym;
                    let bad = String::from_utf8(bad).unwrap();
                    assert!(DeviceId::parse(&bad).is_err(), "{bad} accepted");
                }
            }
        }
    }

    #[test]
    fn malformed_ids_are_rejected() {
        let text = key(1).id().to_string();
        for bad in [
            "",
            "WASL",
            "WASL-",
            &text[..text.len() - 1],
            &format!("{text}0"),
            &text.replacen("WASL", "WASX", 1),
            &text.replacen("WASL-", "", 1),
            &format!("{}U{}", &text[..10], &text[11..]),
            &format!("{}*{}", &text[..10], &text[11..]),
            "WASL-ÄÄÄÄ",
        ] {
            assert!(DeviceId::parse(bad).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn swapped_neighbours_are_mostly_rejected() {
        let mut caught = 0;
        let mut total = 0;
        for n in 0..20u8 {
            let compact: String = key(n).id().to_string().replace('-', "");
            let b = compact.as_bytes();
            for i in 4..b.len() - 1 {
                if b[i] == b[i + 1] {
                    continue;
                }
                let mut s = b.to_vec();
                s.swap(i, i + 1);
                total += 1;
                if DeviceId::parse(std::str::from_utf8(&s).unwrap()).is_err() {
                    caught += 1;
                }
            }
        }
        // Luhn misses only a `0`/`Z` swap; the hash symbol then catches 31 in 32 of those.
        assert!(caught * 100 >= total * 99, "{caught}/{total}");
    }

    #[test]
    fn key_store_round_trip() {
        let store = MemoryKeyStore::default();
        let a = DeviceKey::load_or_create(&store).unwrap();
        let b = DeviceKey::load_or_create(&store).unwrap();
        assert_eq!(a.public(), b.public());
        assert_eq!(*store.load().unwrap().unwrap(), *a.seed());
    }

    #[test]
    fn signatures_verify_only_for_their_key_and_message() {
        let k = key(3);
        let sig = k.sign(b"hello");
        assert!(verify_signature(&k.public(), b"hello", &sig));
        assert!(!verify_signature(&k.public(), b"hellO", &sig));
        assert!(!verify_signature(&key(4).public(), b"hello", &sig));
        assert!(parse_public_key(&[0u8; 32]).is_err());
    }
}
