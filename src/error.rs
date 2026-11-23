//! Reasons a patch can't be applied.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The game's code at a site isn't what the patch was written against.
    UnexpectedBytes {
        site: &'static str,
        address: *const u8,
        found: Vec<u8>,
        expected: &'static [u8],
    },
    /// No free memory within `jmp rel32` range of the game's code.
    NoCaveMemory,
    /// The cave page has no room left.
    CavePageFull,
    /// A jump target is more than ±2 GiB away.
    OutOfJumpRange,
    /// Changing memory protection failed.
    Protect { address: *const u8 },
    /// A game thread kept stopping inside the code about to be overwritten.
    CodeInUse,
    /// A write failed, and putting back the sites already written failed too, so
    /// only some of a feature's sites are patched.
    PartlyApplied { cause: Box<Error> },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedBytes {
                site,
                address,
                found,
                expected,
            } => write!(
                f,
                "{site} at {address:p} has bytes {} but expected {}",
                Hex(found),
                Hex(expected)
            ),
            Self::NoCaveMemory => {
                f.write_str("no free memory within jump range of the game's code")
            }
            Self::CavePageFull => f.write_str("cave page is full"),
            Self::OutOfJumpRange => f.write_str("jump target out of range"),
            Self::Protect { address } => write!(f, "VirtualProtect failed at {address:p}"),
            Self::CodeInUse => {
                f.write_str("a game thread kept stopping inside the code being patched")
            }
            Self::PartlyApplied { cause } => write!(
                f,
                "{cause}, and undoing the sites already patched failed too, \
                 so the feature is only partly applied"
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Formats bytes as space-separated upper-case hex, e.g. `F3 0F 11`.
pub struct Hex<'a>(pub &'a [u8]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_formatting() {
        assert_eq!(Hex(&[0xF3, 0x0F, 0x01]).to_string(), "F3 0F 01");
        assert_eq!(Hex(&[]).to_string(), "");
    }
}
