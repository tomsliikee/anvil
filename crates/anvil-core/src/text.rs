//! Files as Visual Studio left them. A file is decoded into text with `\n`
//! line breaks for editing and encoded back with its own byte order mark and
//! line endings, so saving an unchanged file gives the same bytes and a new
//! line typed into a CRLF file is CRLF.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// UTF-8, with or without the EF BB BF mark.
    Utf8 { bom: bool },
    /// UTF-16 little endian with its FF FE mark (some older VS files).
    Utf16Le,
    /// Bytes that are not UTF-8 are read as Latin-1, which maps every byte
    /// to one char and back, so nothing is lost.
    Latin1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    Crlf,
    Lf,
    /// Both kinds occur. The text then keeps its `\r\n` as they are and is
    /// written back unchanged; new lines get the more common kind.
    Mixed { majority_crlf: bool },
}

impl Eol {
    /// The line break to type into this file.
    pub fn newline(self) -> &'static str {
        match self {
            Eol::Crlf | Eol::Mixed { majority_crlf: true } => "\r\n",
            Eol::Lf | Eol::Mixed { majority_crlf: false } => "\n",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextFile {
    pub encoding: Encoding,
    pub eol: Eol,
    /// The text with `\n` line breaks (raw breaks when `eol` is `Mixed`).
    pub text: String,
}

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

impl TextFile {
    /// A new file as Visual Studio writes one: UTF-8 with BOM, CRLF.
    pub fn vs_new(text: &str) -> Self {
        TextFile { encoding: Encoding::Utf8 { bom: true }, eol: Eol::Crlf, text: text.replace("\r\n", "\n") }
    }

    pub fn decode(bytes: &[u8]) -> Self {
        let (encoding, raw) = if let Some(rest) = bytes.strip_prefix(UTF8_BOM) {
            match std::str::from_utf8(rest) {
                Ok(s) => (Encoding::Utf8 { bom: true }, s.to_owned()),
                Err(_) => (Encoding::Latin1, latin1(bytes)),
            }
        } else if bytes.starts_with(b"\xFF\xFE") && bytes.len().is_multiple_of(2) {
            let units: Vec<u16> = bytes[2..].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            match String::from_utf16(&units) {
                Ok(s) => (Encoding::Utf16Le, s),
                Err(_) => (Encoding::Latin1, latin1(bytes)),
            }
        } else {
            match std::str::from_utf8(bytes) {
                Ok(s) => (Encoding::Utf8 { bom: false }, s.to_owned()),
                Err(_) => (Encoding::Latin1, latin1(bytes)),
            }
        };
        let crlf = raw.matches("\r\n").count();
        let lf = raw.matches('\n').count() - crlf;
        let eol = match (crlf, lf) {
            (0, 0) => Eol::Crlf,
            (_, 0) => Eol::Crlf,
            (0, _) => Eol::Lf,
            (c, l) => Eol::Mixed { majority_crlf: c >= l },
        };
        let text = if eol == Eol::Crlf { raw.replace("\r\n", "\n") } else { raw };
        // A file without any line break gets CRLF for new ones only if VS
        // would write it so; an empty-ish file without a BOM is most likely
        // from a Unix tool, so it keeps LF.
        let eol = if crlf == 0 && lf == 0 && encoding == (Encoding::Utf8 { bom: false }) { Eol::Lf } else { eol };
        TextFile { encoding, eol, text }
    }

    pub fn encode(&self) -> Vec<u8> {
        let text = match self.eol {
            Eol::Crlf => self.text.replace('\n', "\r\n"),
            Eol::Lf | Eol::Mixed { .. } => self.text.clone(),
        };
        match self.encoding {
            Encoding::Utf8 { bom } => {
                let mut out = if bom { UTF8_BOM.to_vec() } else { Vec::new() };
                out.extend_from_slice(text.as_bytes());
                out
            }
            Encoding::Utf16Le => {
                let mut out = vec![0xFF, 0xFE];
                for unit in text.encode_utf16() {
                    out.extend_from_slice(&unit.to_le_bytes());
                }
                out
            }
            Encoding::Latin1 => text.chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).collect(),
        }
    }

    pub fn has_bom(&self) -> bool {
        matches!(self.encoding, Encoding::Utf8 { bom: true } | Encoding::Utf16Le)
    }

    /// Reads a file from disk.
    pub fn read(path: &std::path::Path) -> std::io::Result<Self> {
        Ok(Self::decode(&std::fs::read(path)?))
    }

    /// Writes through a temporary file next to `path`, so a crash never
    /// leaves half a file behind.
    pub fn write(&self, path: &std::path::Path) -> std::io::Result<()> {
        write_atomic(path, &self.encode())
    }
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.anvil-tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Byte offsets into a text as (line, byte within line), the way
/// `GtkTextBuffer::iter_at_line_index` takes them: one index for many
/// lookups instead of counting characters from the start each time.
#[derive(Clone, Debug)]
pub struct LineIndex {
    starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        LineIndex { starts, len: text.len() }
    }

    /// The line and in-line byte of `byte`, moved back onto a character
    /// boundary of `text` (a range end inside `ä` would otherwise split it).
    pub fn locate(&self, text: &str, byte: usize) -> (usize, usize) {
        let mut b = byte.min(self.len);
        while !text.is_char_boundary(b) {
            b -= 1;
        }
        let line = self.starts.partition_point(|s| *s <= b) - 1;
        (line, b - self.starts[line])
    }
}

/// The byte just past the character that starts at `byte` (a zero-width
/// diagnostic is drawn over one whole character).
pub fn next_char_end(text: &str, byte: usize) -> usize {
    let b = byte.min(text.len());
    text[b..].chars().next().map_or(b, |c| b + c.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_index_finds_lines_and_respects_umlauts() {
        let t = "ab\nGröße\n\nx";
        let ix = LineIndex::new(t);
        assert_eq!(ix.locate(t, 0), (0, 0));
        assert_eq!(ix.locate(t, 3), (1, 0));
        // Byte 6 is inside "ö"; it moves back to its start.
        assert_eq!(ix.locate(t, 6), (1, 2));
        assert_eq!(ix.locate(t, t.len()), (3, 1));
        assert_eq!(ix.locate(t, 999), (3, 1));
        assert_eq!(next_char_end(t, 5), 7);
        assert_eq!(next_char_end(t, t.len()), t.len());
    }

    fn fixture_files() -> Vec<std::path::PathBuf> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() { stack.push(path) } else { out.push(path) }
            }
        }
        out
    }

    #[test]
    fn round_trips_every_fixture_byte_for_byte() {
        let files = fixture_files();
        assert!(files.len() >= 8);
        for path in files {
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(TextFile::decode(&bytes).encode(), bytes, "{}", path.display());
        }
    }

    #[test]
    fn course_files_are_bom_crlf() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doublyLinkedList/doublyLinkedList");
        let f = TextFile::read(&root.join("Person.cs")).unwrap();
        assert_eq!(f.encoding, Encoding::Utf8 { bom: true });
        assert_eq!(f.eol, Eol::Crlf);
        assert!(!f.text.contains('\r'));
    }

    #[test]
    fn text_file_keeps_lf_without_bom() {
        let mut f = TextFile::decode(b"a\nb\n");
        assert_eq!(f.eol, Eol::Lf);
        f.text.insert_str(2, "new\n");
        assert_eq!(f.encode(), b"a\nnew\nb\n");
    }

    #[test]
    fn crlf_file_gets_crlf_for_new_lines() {
        let mut f = TextFile::decode(b"\xEF\xBB\xBFa\r\nb\r\n");
        f.text.push_str("c\n");
        assert_eq!(f.encode(), b"\xEF\xBB\xBFa\r\nb\r\nc\r\n");
    }

    #[test]
    fn mixed_endings_are_kept_as_they_are() {
        let bytes = b"a\r\nb\nc\r\n";
        let f = TextFile::decode(bytes);
        assert_eq!(f.eol, Eol::Mixed { majority_crlf: true });
        assert_eq!(f.eol.newline(), "\r\n");
        assert_eq!(f.encode(), bytes);
    }

    #[test]
    fn utf16_and_latin1_survive() {
        let utf16 = [0xFF, 0xFE, b'h', 0, b'i', 0, b'\r', 0, b'\n', 0];
        assert_eq!(TextFile::decode(&utf16).encode(), utf16);
        let latin1 = b"caf\xE9\r\n";
        let f = TextFile::decode(latin1);
        assert_eq!(f.encoding, Encoding::Latin1);
        assert_eq!(f.text, "caf\u{e9}\n");
        assert_eq!(f.encode(), latin1);
    }

    #[test]
    fn vs_new_is_bom_crlf() {
        assert_eq!(TextFile::vs_new("x\n").encode(), b"\xEF\xBB\xBFx\r\n");
    }
}

/// Bytes another tool wrote (the SDK writes LF on Linux), given the BOM and
/// line endings of the file they replace, so `dotnet sln add` does not turn
/// a CRLF solution into an LF one.
pub fn restyle(new_bytes: &[u8], like: Encoding, eol: Eol) -> Vec<u8> {
    let decoded = TextFile::decode(new_bytes);
    let text = if decoded.eol == Eol::Crlf { decoded.text } else { decoded.text.replace("\r\n", "\n") };
    let eol = match eol {
        Eol::Mixed { majority_crlf: true } => Eol::Crlf,
        Eol::Mixed { majority_crlf: false } => Eol::Lf,
        e => e,
    };
    TextFile { encoding: like, eol, text }.encode()
}

#[cfg(test)]
mod restyle_tests {
    use super::*;

    #[test]
    fn sdk_lf_output_becomes_crlf_again() {
        let sdk = b"<Solution>\n  <Project Path=\"a.csproj\" />\n</Solution>\n";
        assert_eq!(restyle(sdk, Encoding::Utf8 { bom: false }, Eol::Crlf), b"<Solution>\r\n  <Project Path=\"a.csproj\" />\r\n</Solution>\r\n");
        let with_bom = b"\xEF\xBB\xBFx\ny\n";
        assert_eq!(restyle(with_bom, Encoding::Utf8 { bom: true }, Eol::Crlf), b"\xEF\xBB\xBFx\r\ny\r\n");
    }
}
