//! The little CBOR (RFC 8949) the file manifest needs, in its deterministic encoding (§4.2.1):
//! definite lengths, shortest-form heads. The decoder accepts only that encoding and never
//! allocates more than the input it was given, so a hostile length cannot exhaust memory.

pub(crate) const UINT: u8 = 0;
pub(crate) const NINT: u8 = 1;
pub(crate) const BYTES: u8 = 2;
pub(crate) const TEXT: u8 = 3;
pub(crate) const ARRAY: u8 = 4;
pub(crate) const MAP: u8 = 5;

pub(crate) fn head(out: &mut Vec<u8>, major: u8, value: u64) {
    let m = major << 5;
    if value < 24 {
        out.push(m | value as u8);
    } else if value <= 0xff {
        out.push(m | 24);
        out.push(value as u8);
    } else if value <= 0xffff {
        out.push(m | 25);
        out.extend_from_slice(&(value as u16).to_be_bytes());
    } else if value <= 0xffff_ffff {
        out.push(m | 26);
        out.extend_from_slice(&(value as u32).to_be_bytes());
    } else {
        out.push(m | 27);
        out.extend_from_slice(&value.to_be_bytes());
    }
}

pub(crate) fn int(out: &mut Vec<u8>, v: i64) {
    if v >= 0 {
        head(out, UINT, v as u64);
    } else {
        // -1 - n encodes as n.
        head(out, NINT, !(v as u64));
    }
}

pub(crate) fn bytes(out: &mut Vec<u8>, b: &[u8]) {
    head(out, BYTES, b.len() as u64);
    out.extend_from_slice(b);
}

pub(crate) fn text(out: &mut Vec<u8>, s: &str) {
    head(out, TEXT, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

/// A cursor over deterministic CBOR. Every method returns `None` on anything unexpected.
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data, pos: 0 }
    }

    pub(crate) fn at_end(&self) -> bool {
        self.pos == self.data.len()
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }

    /// (major type, argument), refusing indefinite lengths and non-shortest heads.
    pub(crate) fn head(&mut self) -> Option<(u8, u64)> {
        let b = *self.take(1)?.first()?;
        let (major, info) = (b >> 5, b & 0x1f);
        let v = match info {
            0..=23 => u64::from(info),
            24 => {
                let v = u64::from(self.take(1)?[0]);
                (v >= 24).then_some(v)?
            }
            25 => {
                let v = u64::from(u16::from_be_bytes(self.take(2)?.try_into().ok()?));
                (v > 0xff).then_some(v)?
            }
            26 => {
                let v = u64::from(u32::from_be_bytes(self.take(4)?.try_into().ok()?));
                (v > 0xffff).then_some(v)?
            }
            27 => {
                let v = u64::from_be_bytes(self.take(8)?.try_into().ok()?);
                (v > 0xffff_ffff).then_some(v)?
            }
            _ => return None,
        };
        Some((major, v))
    }

    pub(crate) fn expect(&mut self, major: u8) -> Option<u64> {
        let (m, v) = self.head()?;
        (m == major).then_some(v)
    }

    pub(crate) fn uint(&mut self) -> Option<u64> {
        self.expect(UINT)
    }

    pub(crate) fn int(&mut self) -> Option<i64> {
        match self.head()? {
            (UINT, v) => i64::try_from(v).ok(),
            (NINT, v) => i64::try_from(v).ok().map(|n| -1 - n),
            _ => None,
        }
    }

    pub(crate) fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = usize::try_from(self.expect(BYTES)?).ok()?;
        self.take(n)
    }

    pub(crate) fn fixed<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.bytes()?.try_into().ok()
    }

    pub(crate) fn key(&mut self, expected: &str) -> Option<()> {
        let n = usize::try_from(self.expect(TEXT)?).ok()?;
        (self.take(n)? == expected.as_bytes()).then_some(())
    }
}
