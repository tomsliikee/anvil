//! The `<HashCode>` of a `.cd` type: Visual Studio's fingerprint of a
//! type's members, 32 bytes as base64.
//!
//! What is known (*tested* against the five course types and 640 types
//! from 100 public `.cd` files with their sources, 2026-11-18):
//!
//! - It is a 256-bit set; each member sets one bit chosen by its **name**
//!   alone. The same name gives the same bit in every type, project and
//!   member kind (`Id` as field and as property, `Value` in four types with
//!   four different types, `Main` with and without `string[] args`).
//! - Constructors (and destructors) set no bit; a type without other
//!   members is all zero (`DoublyLinkedPersonList`). Overloads share a bit.
//! - Names are case-sensitive; bit `i` is byte `i / 8`, bit `i % 8`.
//!
//! What is not known: the function from a name to its bit. More than a
//! thousand candidate hashes (polynomial, FNV, djb2, .NET Framework's 32-
//! and 64-bit `String.GetHashCode`, CRC32, MD5/SHA bytes, with many
//! reductions) did not reproduce it. So Anvil never rewrites a hash Visual
//! Studio wrote, and computes one only where every name's bit is known:
//! an empty type, or names whose bit a [`Learned`] table holds from
//! diagrams it has read. Everything else keeps its stored value.

use std::collections::{BTreeSet, HashMap};

pub type Bits = [u8; 32];

pub const EMPTY: Bits = [0; 32];

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn to_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(B64[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn from_base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.trim().bytes() {
        if c == b'=' {
            break;
        }
        let v = B64.iter().position(|&b| b == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

pub fn parse(s: &str) -> Option<Bits> {
    from_base64(s)?.try_into().ok()
}

pub fn set_bits(b: &Bits) -> BTreeSet<u8> {
    (0..=255u8).filter(|&i| b[usize::from(i / 8)] >> (i % 8) & 1 == 1).collect()
}

pub fn from_set(set: &BTreeSet<u8>) -> Bits {
    let mut b = EMPTY;
    for &i in set {
        b[usize::from(i / 8)] |= 1 << (i % 8);
    }
    b
}

/// The names that take part: every member except constructors and
/// destructors, each name once.
pub fn hashed_names<'a>(members: impl IntoIterator<Item = &'a crate::csharp::Member>) -> BTreeSet<&'a str> {
    use crate::csharp::MemberKind;
    members.into_iter().filter(|m| !matches!(m.kind, MemberKind::Constructor | MemberKind::Destructor)).map(|m| m.name.as_str()).collect()
}

/// Bits of names learned from diagrams VS wrote: a name's bit is known
/// when a type's hash leaves exactly one candidate for it.
#[derive(Clone, Debug, Default)]
pub struct Learned {
    bits: HashMap<String, u8>,
}

impl Learned {
    pub fn get(&self, name: &str) -> Option<u8> {
        self.bits.get(name).copied()
    }

    pub fn len(&self) -> usize {
        self.bits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }

    /// Learns from types whose stored hash matches their current members in
    /// count; repeated until nothing new is found.
    pub fn learn(&mut self, samples: &[(BTreeSet<String>, Bits)]) {
        loop {
            let before = self.bits.len();
            for (names, hash) in samples {
                let set = set_bits(hash);
                if set.len() > names.len() {
                    continue; // stale: the code lost members since VS saved
                }
                let unknown: Vec<&String> = names.iter().filter(|n| !self.bits.contains_key(*n)).collect();
                let rest: Vec<u8> = set.iter().copied().filter(|b| !names.iter().any(|n| self.bits.get(n) == Some(b))).collect();
                if let ([name], [bit]) = (unknown.as_slice(), rest.as_slice()) {
                    self.bits.insert((*name).clone(), *bit);
                }
            }
            if self.bits.len() == before {
                break;
            }
        }
    }

    /// The hash of a type with these member names, when every name's bit is
    /// known.
    pub fn compute<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Option<Bits> {
        let mut set = BTreeSet::new();
        for n in names {
            set.insert(self.get(n)?);
        }
        Some(from_set(&set))
    }
}

/// The hash Anvil writes for a type: the stored one when VS wrote one,
/// otherwise a computed one when possible (always for an empty type).
pub fn hash_for<'a>(stored: Option<&str>, names: impl IntoIterator<Item = &'a str>, learned: &Learned) -> Option<String> {
    if let Some(s) = stored {
        return Some(s.to_owned());
    }
    learned.compute(names).map(|b| to_base64(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cd::Diagram;
    use crate::csharp;
    use crate::text::TextFile;

    fn course(file: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doublyLinkedList/doublyLinkedList").join(file)
    }

    #[test]
    fn base64_round_trip() {
        for s in ["AAAAAEAAAABABAAAAAQAAAgAAAAIAAAAAAAACAAAAAg=", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", "AgAAAAAAAAAAAAAAAAAAAAAAAAAAAEAAAEBAAAJAAAA="] {
            let b = parse(s).unwrap();
            assert_eq!(to_base64(&b), s);
        }
        assert_eq!(to_base64(b"Man"), "TWFu");
        assert_eq!(to_base64(b"Ma"), "TWE=");
    }

    /// The course diagram: every type sets exactly one bit per distinct
    /// non-constructor member name (the structure the module doc states).
    #[test]
    fn course_bits_match_member_names() {
        let d = Diagram::parse(&std::fs::read(course("ClassDiagram1.cd")).unwrap()).unwrap();
        for shape in d.shapes() {
            let file = TextFile::read(&course(shape.file.as_deref().unwrap())).unwrap();
            let f = csharp::parse(&file.text);
            let t = f.find(&shape.name).unwrap();
            let names = hashed_names(&t.members);
            let bits = set_bits(&parse(shape.hash.as_deref().unwrap()).unwrap());
            assert_eq!(bits.len(), names.len(), "{}", shape.name);
        }
    }

    #[test]
    fn course_empty_list() {
        assert_eq!(to_base64(&Learned::default().compute([]).unwrap()), "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
    }

    #[test]
    fn course_program() {
        // Main sets bit 218 (byte 27, bit 2) in the course and in three
        // public samples with Main() instead of Main(string[] args).
        let mut l = Learned::default();
        let main: BTreeSet<String> = ["Main".to_owned()].into();
        l.learn(&[(main, parse("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAAAAA=").unwrap())]);
        assert_eq!(l.get("Main"), Some(218));
        assert_eq!(to_base64(&l.compute(["Main"]).unwrap()), "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAAAAA=");
    }

    #[test]
    #[ignore = "HashCode name-to-bit function not reproduced (see module doc)"]
    fn course_person() {
        unimplemented!("needs the name hash");
    }

    #[test]
    fn stored_hash_is_kept() {
        assert_eq!(hash_for(Some("XYZ="), ["A"], &Learned::default()).as_deref(), Some("XYZ="));
        assert_eq!(hash_for(None, ["Unknown"], &Learned::default()), None);
    }
}
