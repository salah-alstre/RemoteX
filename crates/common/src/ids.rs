use serde::{Deserialize, Serialize};
use std::fmt;

/// Public device identifier: nine decimal digits, displayed as `123 456 789`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeviceId(String);

impl DeviceId {
    pub const LEN: usize = 9;

    /// Accepts input with spaces, dashes or dots and returns the canonical form.
    pub fn parse(input: &str) -> Option<Self> {
        let digits: String = input
            .chars()
            .filter(|c| !matches!(c, ' ' | '-' | '.' | '\u{a0}'))
            .collect();
        let digits = normalize_digits(&digits);
        if digits.len() == Self::LEN && digits.bytes().all(|b| b.is_ascii_digit()) && !digits.starts_with('0')
        {
            Some(Self(digits))
        } else {
            None
        }
    }

    pub fn from_number(n: u32) -> Self {
        Self(format!("{n:09}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `583 294 814`
    pub fn display(&self) -> String {
        format!("{} {} {}", &self.0[0..3], &self.0[3..6], &self.0[6..9])
    }
}

/// Maps Arabic-Indic and Extended Arabic-Indic digits to ASCII so IDs typed on an Arabic keyboard work.
fn normalize_digits(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{0660}'..='\u{0669}' => char::from(b'0' + (c as u32 - 0x0660) as u8),
            '\u{06F0}'..='\u{06F9}' => char::from(b'0' + (c as u32 - 0x06F0) as u8),
            other => other,
        })
        .collect()
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 128-bit random session capability, hex encoded on display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub [u8; 16]);

impl SessionId {
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spaced_and_arabic_digits() {
        assert_eq!(DeviceId::parse("583 294 814").unwrap().as_str(), "583294814");
        assert_eq!(DeviceId::parse("٥٨٣٢٩٤٨١٤").unwrap().as_str(), "583294814");
        assert_eq!(DeviceId::parse("583-294-814").unwrap().display(), "583 294 814");
    }

    #[test]
    fn rejects_invalid() {
        for bad in [
            "",
            "12345678",
            "1234567890",
            "abc294814",
            "083294814",
            "583 294 81x",
        ] {
            assert!(DeviceId::parse(bad).is_none(), "{bad}");
        }
    }
}
