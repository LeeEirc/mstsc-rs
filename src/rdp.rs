use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

/// Encoding detected when an `.rdp` document is loaded.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RdpEncoding {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

/// A typed RDP value. Unknown type tags are deliberately retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RdpValue {
    Integer(i32),
    String(String),
    Binary(String),
    Unknown { kind: String, value: String },
}

impl RdpValue {
    fn kind(&self) -> &str {
        match self {
            Self::Integer(_) => "i",
            Self::String(_) => "s",
            Self::Binary(_) => "b",
            Self::Unknown { kind, .. } => kind,
        }
    }

    fn render_value(&self) -> String {
        match self {
            Self::Integer(value) => value.to_string(),
            Self::String(value) | Self::Binary(value) | Self::Unknown { value, .. } => {
                value.clone()
            }
        }
    }
}

/// One parsed property line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RdpEntry {
    pub key: String,
    pub value: RdpValue,
    raw: String,
    dirty: bool,
}

impl RdpEntry {
    pub fn normalized_key(&self) -> String {
        normalize_key(&self.key)
    }

    pub fn render(&self) -> String {
        if !self.dirty {
            return self.raw.clone();
        }
        format!(
            "{}:{}:{}",
            self.key,
            self.value.kind(),
            self.value.render_value()
        )
    }
}

/// Parsed lines retain comments, blanks, malformed lines, unknown types and
/// original spellings so callers can round-trip documents without data loss.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RdpLine {
    Entry(RdpEntry),
    Raw(String),
}

/// A lossless-enough `.rdp` model: untouched lines are emitted byte-for-byte
/// after decoding, while overridden properties are rewritten canonically.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RdpDocument {
    lines: Vec<RdpLine>,
    encoding: RdpEncoding,
    newline: String,
    trailing_newline: bool,
}

impl Default for RdpDocument {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            encoding: RdpEncoding::Utf8,
            newline: "\r\n".to_owned(),
            trailing_newline: true,
        }
    }
}

impl RdpDocument {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|source| Error::ReadRdp {
            path: path.to_owned(),
            source,
        })?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let (text, encoding) = decode(bytes)?;
        Ok(Self::parse_with_encoding(&text, encoding))
    }

    pub fn parse(text: &str) -> Self {
        Self::parse_with_encoding(text, RdpEncoding::Utf8)
    }

    fn parse_with_encoding(text: &str, encoding: RdpEncoding) -> Self {
        let newline = if text.contains("\r\n") {
            "\r\n"
        } else if text.contains('\n') {
            "\n"
        } else if text.contains('\r') {
            "\r"
        } else {
            "\r\n"
        };
        let trailing_newline = text.ends_with('\n') || text.ends_with('\r');
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut parts: Vec<&str> = normalized.split('\n').collect();
        if trailing_newline && parts.last() == Some(&"") {
            parts.pop();
        }

        let lines = parts
            .into_iter()
            .map(|line| parse_line(line).unwrap_or_else(|| RdpLine::Raw(line.to_owned())))
            .collect();

        Self {
            lines,
            encoding,
            newline: newline.to_owned(),
            trailing_newline,
        }
    }

    pub fn encoding(&self) -> RdpEncoding {
        self.encoding
    }

    pub fn lines(&self) -> &[RdpLine] {
        &self.lines
    }

    pub fn get(&self, key: &str) -> Option<&RdpValue> {
        let wanted = normalize_key(key);
        self.lines.iter().rev().find_map(|line| match line {
            RdpLine::Entry(entry) if entry.normalized_key() == wanted => Some(&entry.value),
            _ => None,
        })
    }

    pub fn get_string(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(RdpValue::String(value)) => Some(value),
            _ => None,
        }
    }

    pub fn get_integer(&self, key: &str) -> Option<i32> {
        match self.get(key) {
            Some(RdpValue::Integer(value)) => Some(*value),
            _ => None,
        }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Removes all occurrences of a property. This is useful when a later
    /// credential representation must not coexist with an older one.
    pub fn remove_all(&mut self, key: &str) {
        let wanted = normalize_key(key);
        self.lines.retain(|line| match line {
            RdpLine::Entry(entry) => entry.normalized_key() != wanted,
            RdpLine::Raw(_) => true,
        });
    }

    pub fn set_integer(&mut self, key: impl Into<String>, value: i32) {
        self.set_value(key.into(), RdpValue::Integer(value));
    }

    pub fn set_string(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.set_value(key.into(), RdpValue::String(value.into()));
    }

    pub fn set_binary(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.set_value(key.into(), RdpValue::Binary(value.into()));
    }

    pub fn set_assignment(&mut self, assignment: &str) -> Result<()> {
        let Some(RdpLine::Entry(mut entry)) = parse_line(assignment) else {
            return Err(Error::InvalidRdpAssignment(assignment.to_owned()));
        };
        entry.dirty = true;
        self.set_value(entry.key, entry.value);
        Ok(())
    }

    fn set_value(&mut self, key: String, value: RdpValue) {
        let wanted = normalize_key(&key);
        if let Some(entry) = self.lines.iter_mut().rev().find_map(|line| match line {
            RdpLine::Entry(entry) if entry.normalized_key() == wanted => Some(entry),
            _ => None,
        }) {
            entry.value = value;
            entry.dirty = true;
            return;
        }

        self.lines.push(RdpLine::Entry(RdpEntry {
            raw: String::new(),
            key,
            value,
            dirty: true,
        }));
    }

    pub fn render(&self) -> String {
        let mut text = self
            .lines
            .iter()
            .map(|line| match line {
                RdpLine::Entry(entry) => entry.render(),
                RdpLine::Raw(raw) => raw.clone(),
            })
            .collect::<Vec<_>>()
            .join(&self.newline);
        if self.trailing_newline && (!self.lines.is_empty() || !text.is_empty()) {
            text.push_str(&self.newline);
        }
        text
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let text = self.render();
        match self.encoding {
            RdpEncoding::Utf8 => text.into_bytes(),
            RdpEncoding::Utf8Bom => {
                let mut bytes = vec![0xef, 0xbb, 0xbf];
                bytes.extend_from_slice(text.as_bytes());
                bytes
            }
            RdpEncoding::Utf16Le => {
                let mut bytes = vec![0xff, 0xfe];
                for unit in text.encode_utf16() {
                    bytes.extend_from_slice(&unit.to_le_bytes());
                }
                bytes
            }
            RdpEncoding::Utf16Be => {
                let mut bytes = vec![0xfe, 0xff];
                for unit in text.encode_utf16() {
                    bytes.extend_from_slice(&unit.to_be_bytes());
                }
                bytes
            }
        }
    }
}

fn parse_line(line: &str) -> Option<RdpLine> {
    let mut fields = line.splitn(3, ':');
    let key = fields.next()?.trim();
    let kind = fields.next()?.trim();
    let raw_value = fields.next()?;
    if key.is_empty() || kind.is_empty() {
        return None;
    }

    let value = match kind.to_ascii_lowercase().as_str() {
        "i" => RdpValue::Integer(raw_value.trim().parse().ok()?),
        "s" => RdpValue::String(raw_value.to_owned()),
        "b" => RdpValue::Binary(raw_value.to_owned()),
        _ => RdpValue::Unknown {
            kind: kind.to_owned(),
            value: raw_value.to_owned(),
        },
    };
    Some(RdpLine::Entry(RdpEntry {
        key: key.to_owned(),
        value,
        raw: line.to_owned(),
        dirty: false,
    }))
}

fn normalize_key(key: &str) -> String {
    key.trim().to_ascii_lowercase()
}

fn decode(bytes: &[u8]) -> Result<(String, RdpEncoding)> {
    if let Some(rest) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8(rest.to_vec())
            .map(|text| (text, RdpEncoding::Utf8Bom))
            .map_err(|_| Error::InvalidRdpEncoding);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return decode_utf16(rest, true).map(|text| (text, RdpEncoding::Utf16Le));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return decode_utf16(rest, false).map(|text| (text, RdpEncoding::Utf16Be));
    }
    if let Ok(text) = String::from_utf8(bytes.to_vec()) {
        return Ok((text, RdpEncoding::Utf8));
    }

    // Some generated files omit the BOM. NUL placement is a reliable signal
    // for ordinary ASCII property names encoded as UTF-16.
    if bytes.len() >= 4 && bytes.len().is_multiple_of(2) {
        let even_nuls = bytes.iter().step_by(2).filter(|&&byte| byte == 0).count();
        let odd_nuls = bytes
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|&&byte| byte == 0)
            .count();
        if odd_nuls > bytes.len() / 8 {
            return decode_utf16(bytes, true).map(|text| (text, RdpEncoding::Utf16Le));
        }
        if even_nuls > bytes.len() / 8 {
            return decode_utf16(bytes, false).map(|text| (text, RdpEncoding::Utf16Be));
        }
    }

    Err(Error::InvalidRdpEncoding)
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Result<String> {
    if !bytes.len().is_multiple_of(2) {
        return Err(Error::InvalidRdpEncoding);
    }
    let units = bytes.chunks_exact(2).map(|pair| {
        if little_endian {
            u16::from_le_bytes([pair[0], pair[1]])
        } else {
            u16::from_be_bytes([pair[0], pair[1]])
        }
    });
    std::char::decode_utf16(units)
        .collect::<std::result::Result<String, _>>()
        .map_err(|_| Error::InvalidRdpEncoding)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_unknown_and_overrides_last_duplicate() {
        let input = "full address:s:old\r\nx-vendor:z:a:b\r\nfull address:s:new\r\n\r\n";
        let mut doc = RdpDocument::parse(input);
        assert_eq!(doc.get_string("FULL ADDRESS"), Some("new"));
        doc.set_string("full address", "host:3390");
        assert_eq!(
            doc.render(),
            "full address:s:old\r\nx-vendor:z:a:b\r\nfull address:s:host:3390\r\n\r\n"
        );
    }

    #[test]
    fn reads_and_reemits_utf16le() {
        let text = "username:s:测试用户\r\nredirectclipboard:i:1\r\n";
        let mut bytes = vec![0xff, 0xfe];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let doc = RdpDocument::from_bytes(&bytes).unwrap();
        assert_eq!(doc.encoding(), RdpEncoding::Utf16Le);
        assert_eq!(doc.get_string("username"), Some("测试用户"));
        assert_eq!(doc.to_bytes(), bytes);
    }

    #[test]
    fn assignment_keeps_colons_in_string_values() {
        let mut doc = RdpDocument::default();
        doc.set_assignment("full address:s:server.example:3390")
            .unwrap();
        assert_eq!(doc.get_string("full address"), Some("server.example:3390"));
    }

    #[test]
    fn removes_all_duplicate_properties_only() {
        let mut doc =
            RdpDocument::parse("password 51:b:first\r\nx-vendor:z:keep\r\npassword 51:b:last\r\n");
        doc.remove_all("PASSWORD 51");
        assert_eq!(doc.render(), "x-vendor:z:keep\r\n");
    }
}
