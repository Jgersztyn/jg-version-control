//! Object IDs: 20-byte SHA-1 hashes, identical to git's.

use anyhow::{Result, bail};
use serde::{Serialize, Serializer};
use std::fmt;

/// A 20-byte SHA-1 object id. `Copy` because it is small and used everywhere.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid(pub [u8; 20]);

impl Oid {
    /// The all-zero id git uses to mean "no object" (e.g. in reflogs).
    pub const ZERO: Oid = Oid([0u8; 20]);

    /// Hash raw bytes with SHA-1.
    pub fn hash(data: &[u8]) -> Oid {
        Oid(sha1_smol::Sha1::from(data).digest().bytes())
    }

    /// Hash an object the way git does: `"<kind> <len>\0<data>"`.
    pub fn hash_object(kind: &str, data: &[u8]) -> Oid {
        let mut h = sha1_smol::Sha1::new();
        h.update(format!("{} {}\0", kind, data.len()).as_bytes());
        h.update(data);
        Oid(h.digest().bytes())
    }

    pub fn from_hex(s: &str) -> Result<Oid> {
        let s = s.trim();
        if s.len() != 40 {
            bail!("invalid object id '{s}'");
        }
        let mut out = [0u8; 20];
        for i in 0..20 {
            out[i] =
                u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| anyhow::anyhow!("invalid object id '{s}'"))?;
        }
        Ok(Oid(out))
    }

    pub fn from_bytes(b: &[u8]) -> Result<Oid> {
        let arr: [u8; 20] = b.try_into().map_err(|_| anyhow::anyhow!("bad oid length"))?;
        Ok(Oid(arr))
    }

    pub fn to_hex(&self) -> String {
        crate::util::hex(&self.0)
    }

    /// Abbreviated id used in human output.
    pub fn short(&self) -> String {
        self.to_hex()[..7].to_string()
    }

    pub fn is_zero(&self) -> bool {
        *self == Oid::ZERO
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({})", self.short())
    }
}

/// Serialize as a hex string in JSON output.
impl Serialize for Oid {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_blob_matches_git() {
        // `git hash-object -t blob /dev/null`
        assert_eq!(Oid::hash_object("blob", b"").to_hex(), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    }

    #[test]
    fn hex_roundtrip() {
        let o = Oid::hash(b"abc");
        assert_eq!(Oid::from_hex(&o.to_hex()).unwrap(), o);
    }
}
