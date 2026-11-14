#!/usr/bin/env python3
"""Decrypt a Wasl / Drive Sync vault folder (format v1 or v2) on a desktop.

Download the encrypted pair's folder from Google Drive (drive.google.com, right click, Download,
then unzip) and run:

    python3 decrypt_vault.py <vault_folder> <output_dir> [--passphrase PASSPHRASE] [--list] [--overwrite]

The folder must contain the plaintext manifest `vault.json`; its "format" and "version" say which
vault format it is. Names and contents are decrypted recursively into <output_dir>, every
authentication tag is verified, and a summary is printed.

Requirements: Python 3.8+ and the `cryptography` package (`pip install cryptography`). Vault v2
also needs AES-SIV and Argon2id: `cryptography` 44 or newer has both (with an older one,
`argon2-cffi` provides Argon2id). The optional `blake3` package (`pip install blake3`) adds v2's
whole-file BLAKE3 and chunk-id checks on top of the authentication tags, which are always verified.
Format specification: docs/ENCRYPTION.md (format v1, and "Format v2").

Exit codes: 0 success, 1 usage or manifest problem, 2 wrong passphrase, 3 some files failed.
"""

import argparse
import base64
import binascii
import getpass
import hashlib
import hmac
import json
import os
import re
import sys

try:
    from cryptography.exceptions import InvalidTag
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM
except ImportError:  # pragma: no cover
    sys.stderr.write("The 'cryptography' package is required: pip install cryptography\n")
    sys.exit(1)

try:  # vault v2 only
    from cryptography.hazmat.primitives import hashes
    from cryptography.hazmat.primitives.ciphers.aead import AESSIV
    from cryptography.hazmat.primitives.kdf.hkdf import HKDF
except ImportError:  # pragma: no cover
    AESSIV = None

try:
    import blake3 as _blake3
except ImportError:  # pragma: no cover
    _blake3 = None

MAGIC = b"DSV1"
CHUNK = 1 << 20
TAG = 16
NONCE = 12
FILE_NONCE = 8
NAME_AAD = b"drivesync-name-v1"
NAME_NONCE_LABEL = b"name-nonce"
CHUNK_AAD = b"drivesync-chunk-v1"
CHECK_MESSAGE = b"drivesync-vault-check-v1"
SUFFIX = ".dsv"
MANIFEST = "vault.json"
FORMAT = "drivesync-vault"
VERSION = 1
KDF = "PBKDF2-HMAC-SHA256"
B64URL = re.compile(r"^[A-Za-z0-9_-]+$")


class VaultError(Exception):
    pass


# ------------------------------------------------------------------------------------ primitives

def derive_keys(passphrase: str, salt: bytes, iterations: int):
    """PBKDF2-HMAC-SHA256(passphrase UTF-8, salt, iterations, 64 bytes) -> (keyEnc, keyName)."""
    master = hashlib.pbkdf2_hmac("sha256", passphrase.encode("utf-8"), salt, iterations, dklen=64)
    return master[:32], master[32:]


def key_check(key_name: bytes) -> bytes:
    return hmac.new(key_name, CHECK_MESSAGE, hashlib.sha256).digest()


def name_nonce(key_name: bytes, parent_token: str, name: str) -> bytes:
    msg = NAME_NONCE_LABEL + b"\x00" + parent_token.encode("utf-8") + b"\x00" + name.encode("utf-8")
    return hmac.new(key_name, msg, hashlib.sha256).digest()[:NONCE]


def encrypt_name(key_enc: bytes, key_name: bytes, parent_token: str, name: str) -> str:
    """Reference implementation (not used for decryption): deterministic per (parent, name)."""
    nonce = name_nonce(key_name, parent_token, name)
    ct = AESGCM(key_enc).encrypt(nonce, name.encode("utf-8"), NAME_AAD)
    return base64.urlsafe_b64encode(nonce + ct).rstrip(b"=").decode("ascii") + SUFFIX


def decrypt_name(aes: AESGCM, encrypted: str):
    """Plaintext name, or None when the name is not a valid vault name for this key."""
    if not encrypted.endswith(SUFFIX) or len(encrypted) <= len(SUFFIX):
        return None
    body = encrypted[: -len(SUFFIX)]
    if not B64URL.match(body):
        return None
    try:
        raw = base64.urlsafe_b64decode(body + "=" * (-len(body) % 4))
    except (ValueError, binascii.Error):
        return None
    if len(raw) < NONCE + TAG:
        return None
    try:
        plain = aes.decrypt(raw[:NONCE], raw[NONCE:], NAME_AAD)
    except InvalidTag:
        return None
    try:
        text = plain.decode("utf-8")
    except UnicodeDecodeError:
        return None
    if not text or "/" in text or "\x00" in text or text in (".", ".."):
        return None
    return text


def decrypt_stream(aes: AESGCM, src, dst) -> int:
    """Decrypts file object src into file object dst. Returns plaintext bytes written."""
    header = src.read(4 + FILE_NONCE)
    if len(header) < 4 + FILE_NONCE:
        raise VaultError("shorter than the 12-byte header")
    if header[:4] != MAGIC:
        raise VaultError("unknown format or version (magic %r)" % header[:4])
    file_nonce = header[4:]
    index = 0
    total = 0
    buf = src.read(CHUNK + TAG)
    while True:
        nxt = src.read(1) if len(buf) == CHUNK + TAG else b""
        last = len(nxt) == 0
        if len(buf) < TAG:
            raise VaultError("truncated: no complete chunk %d" % index)
        iv = file_nonce + index.to_bytes(4, "big")
        aad = CHUNK_AAD + index.to_bytes(4, "big") + (b"\x01" if last else b"\x00")
        try:
            plain = aes.decrypt(iv, buf, aad)
        except InvalidTag:
            if last:
                raise VaultError("authentication failed on the final chunk %d (modified or truncated)" % index)
            raise VaultError("authentication failed on chunk %d (modified)" % index)
        dst.write(plain)
        total += len(plain)
        if last:
            break
        buf = nxt + src.read(CHUNK + TAG - 1)
        index += 1
    return total


def decrypt_file(aes: AESGCM, src_path: str, dst_path: str) -> int:
    tmp = dst_path + ".dsv-partial"
    try:
        with open(src_path, "rb") as src, open(tmp, "wb") as dst:
            total = decrypt_stream(aes, src, dst)
        os.replace(tmp, dst_path)
        return total
    except BaseException:
        try:
            os.remove(tmp)
        except OSError:
            pass
        raise


# ------------------------------------------------------------------------------------ vault v2

V2_FORMAT = "wasl-vault"
V2_VERSION = 2
V2_KDF = "argon2id"
V2_SUFFIX = ".w2"
V2_INFO_NAME = b"wasl-v2-name"
V2_INFO_META = b"wasl-v2-meta"
V2_INFO_CHUNK = b"wasl-v2-chunk"
V2_INFO_CID = b"wasl-v2-cid"
V2_INFO_DIR = b"wasl-v2-dir"
V2_INFO_CHECK = b"wasl-v2-check"
V2_CHECK_MESSAGE = b"wasl-vault-check-v2"
V2_MANIFEST_MAGIC = b"W2M1"
V2_MANIFEST_AAD = b"wasl-v2-manifest"
V2_META_AAD = b"wasl-v2-meta"
V2_CHUNK_MAGIC = b"W2C1"
V2_CHUNK_AAD = b"wasl-v2-chunk"
V2_DIR_MAGIC = b"W2D1"
V2_DIR_TAG_LABEL = b"wasl-v2-dir"
V2_DIR_MARKER = ".wasl-dir"
V2_CHUNKS_DIR = ".wasl-chunks"
V2_PARTIAL = ".wasl-partial"
V2_ROOT_DIR_ID = bytes(16)
V2_MAX_CHUNK = 4 << 20
V2_MAX_CHUNKS = 1 << 20
V2_MAX_MANIFEST = 64 << 20
V2_META_PROPERTY = "wasl.m"
V2_MAX_PROPERTY = 124
V2_M_RANGE = (19 * 1024, 1024 * 1024)   # KiB
V2_T_RANGE = (1, 10)
V2_P_RANGE = (1, 8)


def b64url(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode("ascii")


def b64url_decode_canonical(text: str):
    """Bytes of canonical unpadded base64url, or None (padding, foreign characters, stray bits)."""
    if not text or not B64URL.match(text) or len(text) % 4 == 1:
        return None
    try:
        raw = base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))
    except (ValueError, binascii.Error):
        return None
    return raw if b64url(raw) == text else None


def b64_decode_canonical(text):
    if not isinstance(text, str):
        return None
    try:
        raw = base64.b64decode(text, validate=True)
    except (ValueError, binascii.Error):
        return None
    return raw if base64.b64encode(raw).decode("ascii") == text else None


# Pure-Python Argon2id (RFC 9106, version 0x13): the last resort when neither `cryptography`
# (with an OpenSSL that has Argon2) nor `argon2-cffi` is available. Slow (tens of seconds at the
# reader minimum, minutes at the 64 MiB default) but dependency-free.

_M64 = (1 << 64) - 1
_M32 = (1 << 32) - 1


def _blake2b_long(out_len: int, data: bytes) -> bytes:
    """H' of RFC 9106 section 3.3."""
    prefix = out_len.to_bytes(4, "little")
    if out_len <= 64:
        return hashlib.blake2b(prefix + data, digest_size=out_len).digest()
    r = (out_len + 31) // 32 - 2
    v = hashlib.blake2b(prefix + data).digest()
    out = [v[:32]]
    for _ in range(1, r):
        v = hashlib.blake2b(v).digest()
        out.append(v[:32])
    out.append(hashlib.blake2b(v, digest_size=out_len - 32 * r).digest())
    return b"".join(out)


def _permute(v, i0, i1, i2, i3, i4, i5, i6, i7, i8, i9, i10, i11, i12, i13, i14, i15):
    """BLAKE2b round without message, with the Argon2 multiplication, on 16 words of v."""
    for a, b, c, d in ((i0, i4, i8, i12), (i1, i5, i9, i13), (i2, i6, i10, i14), (i3, i7, i11, i15),
                       (i0, i5, i10, i15), (i1, i6, i11, i12), (i2, i7, i8, i13), (i3, i4, i9, i14)):
        va, vb, vc, vd = v[a], v[b], v[c], v[d]
        va = (va + vb + 2 * (va & _M32) * (vb & _M32)) & _M64
        vd ^= va
        vd = (vd >> 32) | ((vd << 32) & _M64)
        vc = (vc + vd + 2 * (vc & _M32) * (vd & _M32)) & _M64
        vb ^= vc
        vb = (vb >> 24) | ((vb << 40) & _M64)
        va = (va + vb + 2 * (va & _M32) * (vb & _M32)) & _M64
        vd ^= va
        vd = (vd >> 16) | ((vd << 48) & _M64)
        vc = (vc + vd + 2 * (vc & _M32) * (vd & _M32)) & _M64
        vb ^= vc
        vb = (vb >> 63) | ((vb << 1) & _M64)
        v[a], v[b], v[c], v[d] = va, vb, vc, vd


_ROWS = [tuple(range(16 * i, 16 * i + 16)) for i in range(8)]
_COLS = [tuple(2 * i + 16 * k + e for k in range(8) for e in (0, 1)) for i in range(8)]


def _compress(x, y):
    """G(X, Y) on blocks of 128 words."""
    r = [a ^ b for a, b in zip(x, y)]
    q = r[:]
    for idx in _ROWS:
        _permute(q, *idx)
    for idx in _COLS:
        _permute(q, *idx)
    return [a ^ b for a, b in zip(q, r)]


def _block_words(b: bytes):
    return [int.from_bytes(b[i:i + 8], "little") for i in range(0, 1024, 8)]


def argon2id_pure(password: bytes, salt: bytes, t: int, m: int, p: int, tag_len: int = 32,
                  secret: bytes = b"", ad: bytes = b"") -> bytes:
    le32 = lambda n: n.to_bytes(4, "little")
    h0 = hashlib.blake2b(le32(p) + le32(tag_len) + le32(m) + le32(t) + le32(0x13) + le32(2)
                         + le32(len(password)) + password + le32(len(salt)) + salt
                         + le32(len(secret)) + secret + le32(len(ad)) + ad).digest()
    m_ = 4 * p * (m // (4 * p))
    q = m_ // p
    sl = q // 4
    mem = [[None] * q for _ in range(p)]
    for lane in range(p):
        mem[lane][0] = _block_words(_blake2b_long(1024, h0 + le32(0) + le32(lane)))
        mem[lane][1] = _block_words(_blake2b_long(1024, h0 + le32(1) + le32(lane)))
    zero = [0] * 128
    for r in range(t):
        for s in range(4):
            for lane in range(p):
                di = r == 0 and s < 2
                start = 2 if (r == 0 and s == 0) else 0
                if di:
                    inp = [r, lane, s, m_, t, 2, 0] + [0] * 121
                    addresses = None
                    if start == 2:
                        inp[6] += 1
                        addresses = _compress(zero, _compress(zero, inp))
                row = mem[lane]
                for idx in range(start, sl):
                    j = s * sl + idx
                    prev = row[j - 1] if j > 0 else row[q - 1]
                    if di:
                        if idx % 128 == 0:
                            inp[6] += 1
                            addresses = _compress(zero, _compress(zero, inp))
                        rand = addresses[idx % 128]
                    else:
                        rand = prev[0]
                    j1, j2 = rand & _M32, rand >> 32
                    ref_lane = lane if (r == 0 and s == 0) else j2 % p
                    same = ref_lane == lane
                    if r == 0:
                        w = s * sl + idx - 1 if same else s * sl + (-1 if idx == 0 else 0)
                    else:
                        w = q - sl + idx - 1 if same else q - sl + (-1 if idx == 0 else 0)
                    x = (j1 * j1) >> 32
                    y = (w * x) >> 32
                    rel = w - 1 - y
                    begin = 0 if (r == 0 or s == 3) else (s + 1) * sl
                    ref = mem[ref_lane][(begin + rel) % q]
                    new = _compress(prev, ref)
                    if r > 0:
                        new = [a ^ b for a, b in zip(new, row[j])]
                    row[j] = new
    final = mem[0][q - 1]
    for lane in range(1, p):
        final = [a ^ b for a, b in zip(final, mem[lane][q - 1])]
    return _blake2b_long(tag_len, b"".join(w.to_bytes(8, "little") for w in final))


_ARGON2_NOTE = [False]


def argon2id(passphrase: str, salt: bytes, m: int, t: int, p: int) -> bytes:
    """Argon2id v1.3 -> 32 bytes. `m` is in KiB. Uses `cryptography` when its OpenSSL has Argon2,
    else `argon2-cffi`, else the pure-Python implementation above (slow)."""
    secret = passphrase.encode("utf-8")
    try:
        from cryptography.exceptions import UnsupportedAlgorithm
        from cryptography.hazmat.primitives.kdf.argon2 import Argon2id
        return Argon2id(salt=salt, length=32, iterations=t, lanes=p, memory_cost=m).derive(secret)
    except (ImportError, UnsupportedAlgorithm):
        pass
    try:
        from argon2.low_level import Type, hash_secret_raw
        return hash_secret_raw(secret, salt, time_cost=t, memory_cost=m, parallelism=p, hash_len=32,
                               type=Type.ID, version=19)
    except ImportError:
        pass
    if not _ARGON2_NOTE[0]:
        _ARGON2_NOTE[0] = True
        print("Note:       no native Argon2id (pip install argon2-cffi); using the slow built-in one, "
              "this can take a minute", flush=True)
    return argon2id_pure(secret, salt, t, m, p)


def hkdf(ikm: bytes, info: bytes, length: int) -> bytes:
    return HKDF(algorithm=hashes.SHA256(), length=length, salt=None, info=info).derive(ikm)


class KeysV2:
    def __init__(self, ikm: bytes):
        self.ikm = ikm
        self.k_name = hkdf(ikm, V2_INFO_NAME, 64)
        self.k_meta = hkdf(ikm, V2_INFO_META, 32)
        self.k_chunk = hkdf(ikm, V2_INFO_CHUNK, 32)
        self.k_chunk_id = hkdf(ikm, V2_INFO_CID, 32)
        self.k_dir = hkdf(ikm, V2_INFO_DIR, 32)
        self.key_check = hmac.new(hkdf(ikm, V2_INFO_CHECK, 32), V2_CHECK_MESSAGE, hashlib.sha256).digest()
        self.siv = AESSIV(self.k_name)
        self.meta = AESGCM(self.k_meta)
        self.chunk = AESGCM(self.k_chunk)


def valid_plain_name(name: str) -> bool:
    return bool(name) and name not in (".", "..") and "/" not in name and "\x00" not in name


def encrypt_name_v2(keys: KeysV2, dir_id: bytes, name: str) -> str:
    if not valid_plain_name(name):
        raise ValueError("not a valid vault name: %r" % name)
    return b64url(keys.siv.encrypt(name.encode("utf-8"), [dir_id])) + V2_SUFFIX


def name_siv(encrypted: str):
    """The SIV bytes of an encrypted name (`<base64url>.w2`), or None."""
    if not encrypted.endswith(V2_SUFFIX):
        return None
    raw = b64url_decode_canonical(encrypted[: -len(V2_SUFFIX)])
    return raw if raw is not None and len(raw) > 16 else None


def decrypt_name_v2(keys: KeysV2, dir_id: bytes, encrypted: str):
    """Plaintext name, or None when it is not a name of this vault in the folder `dir_id`."""
    raw = name_siv(encrypted)
    if raw is None:
        return None
    try:
        text = keys.siv.decrypt(raw, [dir_id]).decode("utf-8")
    except (InvalidTag, UnicodeDecodeError, ValueError):
        return None
    return text if valid_plain_name(text) else None


def placement(parent_dir_id: bytes, encrypted_name: str) -> bytes:
    """parent dir id || the name's SIV bytes: what manifests, wasl.m and folder ids are bound to."""
    raw = name_siv(encrypted_name)
    if raw is None:
        raise VaultError("not a vault v2 name: %r" % encrypted_name)
    return parent_dir_id + raw


def dir_tag(keys: KeysV2, folder_placement: bytes, dir_id: bytes) -> bytes:
    return hmac.new(keys.k_dir, V2_DIR_TAG_LABEL + folder_placement + dir_id, hashlib.sha256).digest()


def dir_marker(keys: KeysV2, folder_placement: bytes, dir_id: bytes) -> bytes:
    return V2_DIR_MAGIC + dir_id + dir_tag(keys, folder_placement, dir_id)


def dir_property(keys: KeysV2, folder_placement: bytes, dir_id: bytes) -> str:
    return b64url(dir_id + dir_tag(keys, folder_placement, dir_id))


def parse_dir_marker(keys: KeysV2, folder_placement: bytes, data: bytes):
    """The folder's dir id, or None when the marker is malformed or does not verify here."""
    if len(data) != 52 or data[:4] != V2_DIR_MAGIC:
        return None
    dir_id, tag = data[4:20], data[20:]
    return dir_id if hmac.compare_digest(dir_tag(keys, folder_placement, dir_id), tag) else None


# Deterministic CBOR (RFC 8949 4.2.1), only what the file manifest uses.

def cbor_head(major: int, value: int) -> bytes:
    m = major << 5
    if value < 24:
        return bytes([m | value])
    for info, size in ((24, 1), (25, 2), (26, 4), (27, 8)):
        if value < (1 << (8 * size)):
            return bytes([m | info]) + value.to_bytes(size, "big")
    raise ValueError("integer too large for CBOR")


def cbor_text(s: str) -> bytes:
    b = s.encode("utf-8")
    return cbor_head(3, len(b)) + b


def cbor_bytes(b: bytes) -> bytes:
    return cbor_head(2, len(b)) + b


def cbor_int(v: int) -> bytes:
    return cbor_head(0, v) if v >= 0 else cbor_head(1, -1 - v)


class CborReader:
    def __init__(self, data: bytes):
        self.data = data
        self.pos = 0

    def fail(self, why: str):
        raise VaultError("file manifest content is malformed (%s)" % why)

    def take(self, n: int) -> bytes:
        if n < 0 or self.pos + n > len(self.data):
            self.fail("truncated")
        out = self.data[self.pos:self.pos + n]
        self.pos += n
        return out

    def head(self):
        b = self.take(1)[0]
        major, info = b >> 5, b & 0x1F
        if info < 24:
            return major, info
        sizes = {24: 1, 25: 2, 26: 4, 27: 8}
        if info not in sizes:
            self.fail("indefinite length or reserved")
        n = sizes[info]
        v = int.from_bytes(self.take(n), "big")
        if v < (24 if n == 1 else 1 << (4 * n)):
            self.fail("non-shortest integer")
        return major, v

    def expect(self, major: int) -> int:
        m, v = self.head()
        if m != major:
            self.fail("unexpected type")
        return v

    def int64(self) -> int:
        m, v = self.head()
        if m == 0 and v < (1 << 63):
            return v
        if m == 1 and v < (1 << 63):
            return -1 - v
        self.fail("not an int64")

    def fixed(self, n: int) -> bytes:
        if self.expect(2) != n:
            self.fail("byte string length")
        return self.take(n)

    def key(self, name: str):
        n = self.expect(3)
        if self.take(n) != name.encode("utf-8"):
            self.fail("expected key %r" % name)


def manifest_to_cbor(m: dict) -> bytes:
    out = cbor_head(5, 6)
    out += cbor_text("size") + cbor_head(0, m["size"])
    out += cbor_text("mtime") + cbor_int(m["mtime"])
    out += cbor_text("blake3") + cbor_bytes(m["blake3"])
    out += cbor_text("chunks") + cbor_head(4, len(m["chunks"]))
    for cid, length in m["chunks"]:
        out += cbor_head(4, 2) + cbor_bytes(cid) + cbor_head(0, length)
    out += cbor_text("file_id") + cbor_bytes(m["file_id"])
    out += cbor_text("version") + cbor_head(0, m["version"])
    return out


def manifest_from_cbor(data: bytes) -> dict:
    r = CborReader(data)
    if r.expect(5) != 6:
        r.fail("map size")
    r.key("size")
    size = r.expect(0)
    r.key("mtime")
    mtime = r.int64()
    r.key("blake3")
    digest = r.fixed(32)
    r.key("chunks")
    n = r.expect(4)
    if n > V2_MAX_CHUNKS or n > len(data) // 37:
        raise VaultError("file manifest lists too many chunks")
    chunks = []
    total = 0
    for _ in range(n):
        if r.expect(4) != 2:
            r.fail("chunk entry")
        cid = r.fixed(32)
        length = r.expect(0)
        if not 1 <= length <= V2_MAX_CHUNK:
            raise VaultError("file manifest has a chunk of %d bytes" % length)
        total += length
        chunks.append((cid, length))
    r.key("file_id")
    file_id = r.fixed(16)
    r.key("version")
    version = r.expect(0)
    if r.pos != len(data):
        r.fail("trailing bytes")
    if version < 1:
        raise VaultError("file manifest version must be at least 1")
    if total != size:
        raise VaultError("file manifest chunks add up to %d bytes, not %d" % (total, size))
    return {"file_id": file_id, "size": size, "mtime": mtime, "blake3": digest, "chunks": chunks,
            "version": version}


def seal_manifest(keys: KeysV2, at: bytes, m: dict, nonce: bytes) -> bytes:
    """`at` is the file's placement (see placement())."""
    return V2_MANIFEST_MAGIC + nonce + keys.meta.encrypt(nonce, manifest_to_cbor(m), V2_MANIFEST_AAD + at)


def open_manifest(keys: KeysV2, at: bytes, data: bytes) -> dict:
    if len(data) > V2_MAX_MANIFEST:
        raise VaultError("file manifest is too large")
    if data[:4] != V2_MANIFEST_MAGIC:
        raise VaultError("not a vault v2 file manifest (unknown format or version)")
    if len(data) < 4 + NONCE + TAG:
        raise VaultError("file manifest is truncated")
    try:
        plain = keys.meta.decrypt(data[4:16], data[16:], V2_MANIFEST_AAD + at)
    except InvalidTag:
        raise VaultError("file manifest does not verify (modified, truncated, another file's or another vault's)")
    return manifest_from_cbor(plain)


def encode_meta(keys: KeysV2, at: bytes, size: int, mtime: int, digest: bytes, nonce: bytes) -> str:
    plain = size.to_bytes(8, "big") + mtime.to_bytes(8, "big", signed=True) + digest
    value = base64.b64encode(nonce + keys.meta.encrypt(nonce, plain, V2_META_AAD + at)).decode("ascii")
    assert len(V2_META_PROPERTY) + len(value) <= V2_MAX_PROPERTY
    return value


def decode_meta(keys: KeysV2, at: bytes, value: str):
    raw = b64_decode_canonical(value) if len(V2_META_PROPERTY) + len(value) <= V2_MAX_PROPERTY else None
    if raw is None or len(raw) != 12 + 48 + 16:
        raise VaultError("wasl.m property is malformed")
    try:
        plain = keys.meta.decrypt(raw[:12], raw[12:], V2_META_AAD + at)
    except InvalidTag:
        raise VaultError("wasl.m property does not verify")
    return int.from_bytes(plain[:8], "big"), int.from_bytes(plain[8:16], "big", signed=True), plain[16:]


def chunk_id(keys: KeysV2, file_id: bytes, digest: bytes) -> bytes:
    """HMAC-SHA256(kChunkId, file_id || BLAKE3(plaintext)); `digest` is the BLAKE3."""
    return hmac.new(keys.k_chunk_id, file_id + digest, hashlib.sha256).digest()


def chunk_object_path(cid: bytes) -> str:
    return "%s/%02x/%s" % (V2_CHUNKS_DIR, cid[0], b64url(cid))


def seal_chunk(keys: KeysV2, file_id: bytes, cid: bytes, plain: bytes, nonce: bytes) -> bytes:
    return V2_CHUNK_MAGIC + nonce + keys.chunk.encrypt(nonce, plain, V2_CHUNK_AAD + file_id + cid)


def open_chunk(keys: KeysV2, file_id: bytes, cid: bytes, length, data: bytes) -> bytes:
    if len(data) < 32 or len(data) > 32 + V2_MAX_CHUNK or data[:4] != V2_CHUNK_MAGIC:
        raise VaultError("chunk %s is not a vault v2 chunk" % b64url(cid))
    try:
        plain = keys.chunk.decrypt(data[4:16], data[16:], V2_CHUNK_AAD + file_id + cid)
    except InvalidTag:
        raise VaultError("chunk %s does not verify (modified, truncated, or not this file's)" % b64url(cid))
    if length is not None and len(plain) != length:
        raise VaultError("chunk %s does not match its manifest" % b64url(cid))
    if _blake3 is not None and chunk_id(keys, file_id, _blake3.blake3(plain).digest()) != cid:
        raise VaultError("chunk %s does not match its manifest" % b64url(cid))
    return plain


def read_bounded(path: str, limit: int) -> bytes:
    with open(path, "rb") as f:
        data = f.read(limit + 1)
    if len(data) > limit:
        raise VaultError("%s is larger than the format allows" % os.path.basename(path))
    return data


def decrypt_file_v2(keys: KeysV2, vault_root: str, at: bytes, src_path: str, dst_path: str) -> int:
    m = open_manifest(keys, at, read_bounded(src_path, V2_MAX_MANIFEST))
    tmp = dst_path + V2_PARTIAL
    try:
        h = _blake3.blake3() if _blake3 is not None else None
        total = 0
        with open(tmp, "wb") as dst:
            for cid, length in m["chunks"]:
                path = os.path.join(vault_root, *chunk_object_path(cid).split("/"))
                try:
                    data = read_bounded(path, 32 + V2_MAX_CHUNK)
                except FileNotFoundError:
                    raise VaultError("a piece of the file is missing: %s" % chunk_object_path(cid))
                plain = open_chunk(keys, m["file_id"], cid, length, data)
                if h is not None:
                    h.update(plain)
                dst.write(plain)
                total += len(plain)
        if total != m["size"] or (h is not None and h.digest() != m["blake3"]):
            raise VaultError("the decrypted file does not match its manifest (size or BLAKE3)")
        # Whole nanoseconds: float seconds can land a millisecond off on some systems.
        ns = int(m["mtime"]) * 1_000_000
        try:
            os.utime(tmp, ns=(ns, ns))
        except (OverflowError, OSError, ValueError):
            pass
        os.replace(tmp, dst_path)
        return total
    except BaseException:
        try:
            os.remove(tmp)
        except OSError:
            pass
        raise


def safe_component(name: str) -> bool:
    if name in ("", ".", "..") or os.sep in name or (os.altsep is not None and os.altsep in name):
        return False
    return not os.path.splitdrive(name)[0]


def walk_v2(keys: KeysV2, vault_root: str, src_dir: str, dst_dir: str, rel: str, dir_id: bytes,
            summary, list_only: bool, overwrite: bool):
    try:
        entries = sorted(os.scandir(src_dir), key=lambda e: e.name)
    except OSError as e:
        summary.failed.append((rel or ".", "cannot list folder: %s" % e))
        return
    written_here = set()
    for entry in entries:
        rel_enc = os.path.join(rel, entry.name) if rel else entry.name
        if not rel and entry.name in (MANIFEST, V2_CHUNKS_DIR):
            continue
        if entry.name == V2_DIR_MARKER or entry.name.endswith(V2_PARTIAL):
            continue
        plain = decrypt_name_v2(keys, dir_id, entry.name)
        if plain is None:
            summary.skipped.append((rel_enc, "name is not a vault name for this passphrase"))
            continue
        at = placement(dir_id, entry.name)
        rel_plain = os.path.join(rel, plain) if rel else plain
        if not safe_component(plain):
            summary.failed.append((rel_plain, "name cannot be written safely on this system"))
            continue
        if plain.lower() in written_here:
            summary.failed.append((rel_plain, "another item in this folder decrypts to the same name (case-insensitive)"))
            continue
        written_here.add(plain.lower())
        target = os.path.join(dst_dir, plain)
        if entry.is_dir(follow_symlinks=False):
            try:
                child_id = parse_dir_marker(keys, at, read_bounded(os.path.join(entry.path, V2_DIR_MARKER), 128))
            except (OSError, VaultError):
                child_id = None
            if child_id is None:
                summary.failed.append((rel_plain, "the folder's %s is missing or does not verify, so its contents cannot be named" % V2_DIR_MARKER))
                continue
            summary.dirs += 1
            if list_only:
                print("  [dir]  %s/" % rel_plain)
            else:
                os.makedirs(target, exist_ok=True)
            walk_v2(keys, vault_root, entry.path, target, rel_plain, child_id, summary, list_only, overwrite)
        elif entry.is_file(follow_symlinks=False):
            if list_only:
                try:
                    size = open_manifest(keys, at, read_bounded(entry.path, V2_MAX_MANIFEST))["size"]
                    print("  [file] %s  (%d bytes)" % (rel_plain, size))
                except (OSError, VaultError) as e:
                    print("  [file] %s  (manifest does not verify: %s)" % (rel_plain, e))
                summary.files += 1
                continue
            if os.path.exists(target) and not overwrite:
                summary.skipped.append((rel_plain, "already exists in the output folder (use --overwrite)"))
                continue
            try:
                summary.bytes += decrypt_file_v2(keys, vault_root, at, entry.path, target)
                summary.files += 1
                print("  ok     %s" % rel_plain)
            except VaultError as e:
                summary.failed.append((rel_plain, str(e)))
                sys.stdout.flush()
                print("  FAILED %s: %s" % (rel_plain, e), file=sys.stderr, flush=True)
            except OSError as e:
                summary.failed.append((rel_plain, "I/O error: %s" % e))
                sys.stdout.flush()
                print("  FAILED %s: %s" % (rel_plain, e), file=sys.stderr, flush=True)


def check_vectors(path: str) -> int:
    """Checks the shared test vectors (engine/crates/wasl-crypto/tests/vectors/v2.json) against
    this implementation. Returns the number of checks; raises AssertionError on a mismatch."""
    with open(path, "r", encoding="utf-8") as f:
        v = json.load(f)
    h = bytes.fromhex
    n = 0
    kdf = v["kdf"]
    ikm = argon2id(kdf["passphrase"], h(kdf["salt"]), kdf["m"], kdf["t"], kdf["p"])
    assert ikm.hex() == kdf["ikm"], "argon2id"
    keys = KeysV2(ikm)
    for field in ("k_name", "k_meta", "k_chunk", "k_chunk_id", "k_dir", "key_check"):
        assert getattr(keys, field).hex() == kdf[field], field
        n += 1
    for item in v["names"]:
        assert encrypt_name_v2(keys, h(item["dir_id"]), item["name"]) == item["encrypted"], item["name"]
        assert decrypt_name_v2(keys, h(item["dir_id"]), item["encrypted"]) == item["name"]
        n += 2
    for item in v["bad_names"]:
        assert decrypt_name_v2(keys, h(item["dir_id"]), item["encrypted"]) is None, item
        n += 1
    d = v["dir"]
    at = placement(h(d["parent"]), encrypt_name_v2(keys, h(d["parent"]), d["name"]))
    assert dir_marker(keys, at, h(d["dir_id"])).hex() == d["marker"], "dir marker"
    assert dir_property(keys, at, h(d["dir_id"])) == d["property"], "dir property"
    assert parse_dir_marker(keys, at, h(d["marker"])) == h(d["dir_id"])
    n += 3
    m = v["manifest"]
    at = placement(h(m["parent"]), encrypt_name_v2(keys, h(m["parent"]), m["name"]))
    manifest = {"file_id": h(m["file_id"]), "size": m["size"], "mtime": m["mtime"], "blake3": h(m["blake3"]),
                "chunks": [(h(c[0]), c[1]) for c in m["chunks"]], "version": m["version"]}
    assert manifest_to_cbor(manifest).hex() == m["cbor"], "manifest cbor"
    assert seal_manifest(keys, at, manifest, h(m["nonce"])).hex() == m["sealed"], "manifest sealed"
    assert open_manifest(keys, at, h(m["sealed"])) == manifest
    n += 3
    meta = v["meta"]
    at = placement(h(meta["parent"]), encrypt_name_v2(keys, h(meta["parent"]), meta["name"]))
    assert encode_meta(keys, at, meta["size"], meta["mtime"], h(meta["blake3"]), h(meta["nonce"])) == meta["value"]
    assert decode_meta(keys, at, meta["value"]) == (meta["size"], meta["mtime"], h(meta["blake3"]))
    n += 2
    for c in v["chunks"]:
        plain, fid = h(c["plain"]), h(c["file_id"])
        if _blake3 is not None:
            assert _blake3.blake3(plain).hexdigest() == c["blake3"]
            n += 1
        cid = chunk_id(keys, fid, h(c["blake3"]))
        assert cid.hex() == c["chunk_id"], "chunk id"
        assert chunk_object_path(cid) == c["object_path"]
        assert seal_chunk(keys, fid, cid, plain, h(c["nonce"])).hex() == c["sealed"], "chunk sealed"
        assert open_chunk(keys, fid, cid, len(plain), h(c["sealed"])) == plain
        n += 4
    return n


# ------------------------------------------------------------------------------------ manifest

def load_manifest(folder: str) -> dict:
    path = os.path.join(folder, MANIFEST)
    if not os.path.isfile(path):
        raise VaultError("%s not found in %s (download the whole vault folder from Drive)" % (MANIFEST, folder))
    with open(path, "r", encoding="utf-8") as f:
        try:
            m = json.load(f)
        except ValueError as e:
            raise VaultError("%s is not valid JSON: %s" % (MANIFEST, e))
    if not isinstance(m, dict):
        raise VaultError("%s is not a vault manifest" % MANIFEST)
    if m.get("format") == V2_FORMAT:
        return load_manifest_v2(m)
    if m.get("format") != FORMAT:
        raise VaultError("%s format is %r, expected %r or %r" % (MANIFEST, m.get("format"), FORMAT, V2_FORMAT))
    if m.get("version") != VERSION:
        raise VaultError("vault format version %r is not supported by this tool (supports %d)" % (m.get("version"), VERSION))
    if m.get("kdf") != KDF:
        raise VaultError("KDF %r is not supported (supports %s)" % (m.get("kdf"), KDF))
    try:
        m["_salt"] = base64.b64decode(m["salt"], validate=True)
        m["_keyCheck"] = base64.b64decode(m["keyCheck"], validate=True)
        m["_iterations"] = int(m["iterations"])
    except (KeyError, ValueError, binascii.Error) as e:
        raise VaultError("%s is missing or has invalid fields: %s" % (MANIFEST, e))
    if m["_iterations"] < 1 or len(m["_salt"]) < 8 or len(m["_keyCheck"]) != 32:
        raise VaultError("%s has out-of-range values" % MANIFEST)
    m["_version"] = 1
    return m


def load_manifest_v2(m: dict) -> dict:
    if AESSIV is None:
        raise VaultError("vault v2 needs a newer `cryptography` package (with AESSIV): pip install -U cryptography")
    if m.get("version") != V2_VERSION:
        raise VaultError("vault format version %r is not supported by this tool (supports 1 and %d)" % (m.get("version"), V2_VERSION))
    if m.get("kdf") != V2_KDF:
        raise VaultError("KDF %r is not supported (supports %s)" % (m.get("kdf"), V2_KDF))
    missing = [k for k in ("m", "t", "p", "salt", "keyCheck", "created", "name") if k not in m]
    if missing:
        raise VaultError("%s is missing fields: %s" % (MANIFEST, ", ".join(missing)))
    salt = b64_decode_canonical(m["salt"])
    check = b64_decode_canonical(m["keyCheck"])
    if salt is None or len(salt) != 16 or check is None or len(check) != 32:
        raise VaultError("%s salt or keyCheck is invalid" % MANIFEST)
    params = (m["m"], m["t"], m["p"])
    for value, (lo, hi), label in zip(params, (V2_M_RANGE, V2_T_RANGE, V2_P_RANGE), "mtp"):
        if not isinstance(value, int) or isinstance(value, bool) or not lo <= value <= hi:
            raise VaultError("%s Argon2id %s = %r is outside %d..%d" % (MANIFEST, label, value, lo, hi))
    m["_salt"], m["_keyCheck"], m["_params"], m["_version"] = salt, check, params, 2
    return m


# ------------------------------------------------------------------------------------ walk

class Summary:
    def __init__(self):
        self.dirs = 0
        self.files = 0
        self.bytes = 0
        self.skipped = []   # (path, reason)
        self.failed = []    # (path, reason)


def walk(aes: AESGCM, src_dir: str, dst_dir: str, rel: str, summary: Summary, list_only: bool, overwrite: bool):
    try:
        entries = sorted(os.scandir(src_dir), key=lambda e: e.name)
    except OSError as e:
        summary.failed.append((rel or ".", "cannot list folder: %s" % e))
        return
    written_here = set()
    for entry in entries:
        rel_enc = os.path.join(rel, entry.name) if rel else entry.name
        if not rel and entry.name == MANIFEST:
            continue
        if entry.name.endswith(".dsv-partial"):
            continue
        plain = decrypt_name(aes, entry.name)
        if plain is None:
            summary.skipped.append((rel_enc, "name is not a vault name for this passphrase"))
            continue
        rel_plain = os.path.join(rel, plain) if rel else plain
        if plain.lower() in written_here:
            summary.failed.append((rel_plain, "another item in this folder decrypts to the same name (case-insensitive)"))
            continue
        written_here.add(plain.lower())
        target = os.path.join(dst_dir, plain)
        if entry.is_dir(follow_symlinks=False):
            summary.dirs += 1
            if list_only:
                print("  [dir]  %s/" % rel_plain)
            else:
                os.makedirs(target, exist_ok=True)
            walk(aes, entry.path, target, rel_plain, summary, list_only, overwrite)
        elif entry.is_file(follow_symlinks=False):
            if list_only:
                print("  [file] %s  (%d encrypted bytes)" % (rel_plain, entry.stat().st_size))
                summary.files += 1
                continue
            if os.path.exists(target) and not overwrite:
                summary.skipped.append((rel_plain, "already exists in the output folder (use --overwrite)"))
                continue
            try:
                summary.bytes += decrypt_file(aes, entry.path, target)
                summary.files += 1
                print("  ok     %s" % rel_plain)
            except VaultError as e:
                summary.failed.append((rel_plain, str(e)))
                sys.stdout.flush()
                print("  FAILED %s: %s" % (rel_plain, e), file=sys.stderr, flush=True)
            except OSError as e:
                summary.failed.append((rel_plain, "I/O error: %s" % e))
                sys.stdout.flush()
                print("  FAILED %s: %s" % (rel_plain, e), file=sys.stderr, flush=True)


# ------------------------------------------------------------------------------------ main

def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Decrypt a Wasl vault folder (v1 or v2) downloaded from Google Drive.")
    ap.add_argument("vault_folder", help="folder containing vault.json and the encrypted items")
    ap.add_argument("output_dir", help="where the decrypted tree is written")
    ap.add_argument("--passphrase", help="vault passphrase (prompted when omitted)")
    ap.add_argument("--list", action="store_true", help="only print the decrypted names, write nothing")
    ap.add_argument("--overwrite", action="store_true", help="replace files that already exist in the output folder")
    args = ap.parse_args(argv)

    try:
        manifest = load_manifest(args.vault_folder)
    except VaultError as e:
        print("error: %s" % e, file=sys.stderr)
        return 1

    print("Vault:      %s" % manifest.get("name", "?"))
    print("Created:    %s" % manifest.get("created", "?"))
    if manifest["_version"] == 2:
        print("Format:     v2")
        print("KDF:        Argon2id, m = %d KiB, t = %d, p = %d" % manifest["_params"])
    else:
        print("Format:     v1")
        print("KDF:        %s, %d iterations" % (KDF, manifest["_iterations"]))

    passphrase = args.passphrase if args.passphrase is not None else getpass.getpass("Vault passphrase: ")
    if not passphrase:
        print("error: empty passphrase", file=sys.stderr)
        return 2
    if manifest["_version"] == 2:
        try:
            keys = KeysV2(argon2id(passphrase, manifest["_salt"], *manifest["_params"]))
        except VaultError as e:
            print("error: %s" % e, file=sys.stderr)
            return 1
        ok = hmac.compare_digest(keys.key_check, manifest["_keyCheck"])
    else:
        key_enc, key_name = derive_keys(passphrase, manifest["_salt"], manifest["_iterations"])
        ok = hmac.compare_digest(key_check(key_name), manifest["_keyCheck"])
    if not ok:
        print("error: wrong passphrase for this vault (key check mismatch)", file=sys.stderr)
        return 2
    print("Passphrase: ok")
    if manifest["_version"] == 2 and _blake3 is None:
        print("Note:       the `blake3` package is not installed; tags are verified, whole-file BLAKE3 is not")

    summary = Summary()
    if not args.list:
        os.makedirs(args.output_dir, exist_ok=True)
    print("Decrypting into %s" % os.path.abspath(args.output_dir) if not args.list else "Listing")
    if manifest["_version"] == 2:
        walk_v2(keys, args.vault_folder, args.vault_folder, args.output_dir, "", V2_ROOT_DIR_ID, summary,
                args.list, args.overwrite)
    else:
        walk(AESGCM(key_enc), args.vault_folder, args.output_dir, "", summary, args.list, args.overwrite)

    print("")
    print("Summary: %d folder(s), %d file(s), %d plaintext byte(s)%s" % (
        summary.dirs, summary.files, summary.bytes, " (listed only)" if args.list else ""))
    if summary.skipped:
        print("Skipped %d item(s):" % len(summary.skipped))
        for path, reason in summary.skipped:
            print("  - %s: %s" % (path, reason))
    if summary.failed:
        print("FAILED %d item(s):" % len(summary.failed))
        for path, reason in summary.failed:
            print("  - %s: %s" % (path, reason))
        return 3
    return 0


if __name__ == "__main__":
    sys.exit(main())
