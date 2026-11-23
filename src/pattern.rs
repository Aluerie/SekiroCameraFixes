//! IDA-style byte patterns (`"0F 29 ?? ?? 00"`), parsed at compile time, and searching for them.

/// A byte pattern where each position is either a fixed byte or a wildcard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pattern(&'static [Option<u8>]);

/// Builds a [`Pattern`] from a string literal at compile time. A malformed or empty
/// pattern is a compile error when used in a `const`.
macro_rules! pattern {
    ($text:literal) => {{
        const BYTES: [Option<u8>; $crate::pattern::token_count($text)] =
            match $crate::pattern::parse::<{ $crate::pattern::token_count($text) }>($text) {
                Some(bytes) => bytes,
                None => panic!(concat!("invalid byte pattern: ", $text)),
            };
        $crate::pattern::Pattern::new(&BYTES)
    }};
}

impl Pattern {
    /// Wraps parsed pattern bytes. Panics if `bytes` is empty.
    pub const fn new(bytes: &'static [Option<u8>]) -> Self {
        assert!(!bytes.is_empty(), "empty byte pattern");
        Self(bytes)
    }

    /// Number of bytes the pattern covers.
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Offsets of every position in `haystack` where the pattern matches, in order.
    pub fn matches(self, haystack: &[u8]) -> impl Iterator<Item = usize> {
        let last_start = haystack.len().checked_sub(self.0.len());
        let mut start = 0;
        std::iter::from_fn(move || {
            let last_start = last_start?;
            while start <= last_start {
                let candidate = match self.0[0] {
                    // Anchor on the first fixed byte to skip most positions quickly.
                    Some(first) => {
                        start
                            + haystack[start..=last_start]
                                .iter()
                                .position(|&b| b == first)?
                    }
                    None => start,
                };
                start = candidate + 1;
                if self.matches_at(haystack, candidate) {
                    return Some(candidate);
                }
            }
            None
        })
    }

    fn matches_at(&self, haystack: &[u8], start: usize) -> bool {
        self.0
            .iter()
            .zip(&haystack[start..start + self.0.len()])
            .all(|(want, &have)| want.is_none_or(|w| w == have))
    }
}

/// Number of whitespace-separated tokens in `text`.
pub const fn token_count(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut count = 0;
    let mut in_token = false;
    let mut i = 0;
    while i < bytes.len() {
        let space = bytes[i].is_ascii_whitespace();
        if !space && !in_token {
            count += 1;
        }
        in_token = !space;
        i += 1;
    }
    count
}

/// Parses exactly `N` whitespace-separated tokens, each a two-digit hex byte or a
/// `??`/`?` wildcard. Returns `None` for a bad token, a different token count, or `N == 0`.
pub const fn parse<const N: usize>(text: &str) -> Option<[Option<u8>; N]> {
    if N == 0 {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = [None; N];
    let mut count = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let token = match i - start {
            1 if bytes[start] == b'?' => None,
            2 if bytes[start] == b'?' && bytes[start + 1] == b'?' => None,
            2 => match (hex_digit(bytes[start]), hex_digit(bytes[start + 1])) {
                (Some(high), Some(low)) => Some(high << 4 | low),
                _ => return None,
            },
            _ => return None,
        };
        if count == N {
            return None;
        }
        out[count] = token;
        count += 1;
    }
    if count == N { Some(out) } else { None }
}

const fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bytes_and_wildcards() {
        const P: Pattern = pattern!("C6 86 ?? ?? 00 0f ?");
        assert_eq!(
            P.0,
            [
                Some(0xC6),
                Some(0x86),
                None,
                None,
                Some(0x00),
                Some(0x0F),
                None
            ]
        );
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse::<0>(""), None);
        assert_eq!(parse::<1>("ZZ"), None);
        assert_eq!(parse::<1>("123"), None);
        assert_eq!(parse::<2>("AA"), None);
        assert_eq!(parse::<1>("AA BB"), None);
        assert_eq!(token_count("  AA\t?? BB "), 3);
    }

    fn all(p: Pattern, haystack: &[u8]) -> Vec<usize> {
        p.matches(haystack).collect()
    }

    #[test]
    fn finds_every_match_in_order() {
        let p = pattern!("AA ?? CC");
        let hay = [0x00, 0xAA, 0x01, 0xCC, 0xAA, 0x02, 0xCC];
        assert_eq!(all(p, &hay), [1, 4]);
    }

    #[test]
    fn overlapping_matches() {
        let p = pattern!("AA AA");
        assert_eq!(all(p, &[0xAA, 0xAA, 0xAA]), [0, 1]);
    }

    #[test]
    fn finds_match_at_very_end() {
        let p = pattern!("AA BB");
        assert_eq!(all(p, &[0x00, 0xAA, 0xAA, 0xBB]), [2]);
    }

    #[test]
    fn leading_wildcard() {
        let p = pattern!("?? BB");
        assert_eq!(all(p, &[0xBB, 0x01, 0xBB]), [1]);
    }

    #[test]
    fn no_match() {
        let p = pattern!("AA BB");
        assert_eq!(all(p, &[0xAA]), []);
        assert_eq!(all(p, &[0xAA, 0xAA, 0xCC]), []);
    }
}
