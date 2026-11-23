//! Patch sites in `sekiro.exe`.
//!
//! Patterns, offsets and injected code are taken verbatim from uberhalit's
//! "Sekiro FPS Unlocker and More" (MIT), `GameData.cs`. The disassembly listings
//! below are uberhalit's, from game version 1.2.0.0+. The `expected` bytes are an
//! extra safety check that the original tool doesn't have: a site is only patched
//! when the game's code there is exactly what the injected code was written against.

use crate::pattern::Pattern;

/// How a site is modified once found.
#[derive(Debug, Clone, Copy)]
pub enum Patch {
    /// Overwrite the bytes at the site in place.
    Bytes(&'static [u8]),
    /// Replace the first `overwrite_len` bytes at the site with a `jmp` to newly
    /// allocated code (padded with `nop`s). That code runs `shellcode` and then
    /// jumps back to `site + overwrite_len`.
    Cave {
        overwrite_len: usize,
        shellcode: &'static [u8],
    },
}

/// One location in the game's code that gets patched.
#[derive(Debug, Clone, Copy)]
pub struct Site {
    pub name: &'static str,
    /// Pattern searched over the executable sections of `sekiro.exe`; the first match is used.
    pub pattern: Pattern,
    /// Distance from the start of the pattern match to the patched address.
    pub offset: usize,
    /// Bytes the game must have at the patched address before patching.
    pub expected: &'static [u8],
    pub patch: Patch,
}

// ---------------------------------------------------------------------------
// "Disable camera auto rotate on movement"
// ---------------------------------------------------------------------------

/// Camera pitch. xmm4 holds the new pitch from a calculation, `[rsi+170]` the
/// current one from the mouse, so xmm4 is reloaded with the current pitch.
///
/// ```text
/// 0F29A5 70080000  movaps [rbp+870],xmm4   <- overwritten
/// 0F29A5 80080000  movaps [rbp+880],xmm4   <- return here
/// 0F29A6 70010000  movaps [rsi+170],xmm4   ; camPitch = newCamPitch
/// EB 1C            jmp ...
/// F3:0F108E 74010000 movss xmm1,[rsi+174]
/// ```
pub const CAMADJUST_PITCH: Site = Site {
    name: "camera pitch",
    pattern: pattern!("0F 29 ?? ?? ?? 00 00 0F 29 ?? ?? ?? 00 00 0F 29 ?? ?? ?? 00 00 EB ?? F3"),
    offset: 0,
    expected: &[
        0x0F, 0x29, 0xA5, 0x70, 0x08, 0x00, 0x00, // movaps [rbp+870],xmm4
        0x0F, 0x29, 0xA5, 0x80, 0x08, 0x00, 0x00, // movaps [rbp+880],xmm4
        0x0F, 0x29, 0xA6, 0x70, 0x01, 0x00, 0x00, // movaps [rsi+170],xmm4
    ],
    patch: Patch::Cave {
        overwrite_len: 7,
        shellcode: &[
            0x0F, 0x28, 0xA6, 0x70, 0x01, 0x00, 0x00, // movaps xmm4,[rsi+170]
            0x0F, 0x29, 0xA5, 0x70, 0x08, 0x00, 0x00, // movaps [rbp+870],xmm4
        ],
    },
};

/// Automatic camera yaw adjustment on Z-axis movement. xmm0 holds the new yaw,
/// `[rsi+174]` the yaw before moving, so xmm0 is reloaded with the old yaw.
///
/// ```text
/// E8 6F60FFFF        call ...
/// F3:0F1186 74010000 movss [rsi+174],xmm0   <- overwritten ; camYaw = newCamYaw
/// 80BE A3020000 00   cmp byte [rsi+2A3],0   <- return here
/// 0F84 2F020000      je ...
/// ```
pub const CAMADJUST_YAW_Z: Site = Site {
    name: "camera yaw (Z)",
    pattern: pattern!("E8 ?? ?? ?? ?? F3 ?? ?? ?? ?? ?? 00 00 80 ?? ?? ?? 00 00 00 0F 84"),
    offset: 5,
    expected: &[0xF3, 0x0F, 0x11, 0x86, 0x74, 0x01, 0x00, 0x00], // movss [rsi+174],xmm0
    patch: Patch::Cave {
        overwrite_len: 8,
        shellcode: &[
            0xF3, 0x0F, 0x10, 0x86, 0x74, 0x01, 0x00, 0x00, // movss xmm0,[rsi+174]
            0xF3, 0x0F, 0x11, 0x86, 0x74, 0x01, 0x00, 0x00, // movss [rsi+174],xmm0
        ],
    },
};

/// Automatic camera pitch adjustment on XY-axis movement. `[rax]` holds the new
/// pitch, `[rsi+170]` the pitch before moving, so both xmm0 and `[rax]` get the
/// old pitch. The original tool skips this site for controller users because it
/// breaks pitch and other stick controls there.
///
/// ```text
/// F3:0F1000          movss xmm0,[rax]        <- overwritten ; newCamPitch
/// F3:0F1186 70010000 movss [rsi+170],xmm0    <- overwritten ; camPitch
/// F3:0F1085 E4120000 movss xmm0,[rbp+12E4]   <- return here
/// E8 91BDFFFF        call ...
/// ```
pub const CAMADJUST_PITCH_XY: Site = Site {
    name: "camera pitch (XY)",
    pattern: pattern!(
        "F3 ?? ?? ?? F3 ?? ?? ?? 70 01 00 00 F3 ?? ?? ?? ?? ?? ?? ?? E8 ?? ?? ?? ?? 0F"
    ),
    offset: 0,
    expected: &[
        0xF3, 0x0F, 0x10, 0x00, // movss xmm0,[rax]
        0xF3, 0x0F, 0x11, 0x86, 0x70, 0x01, 0x00, 0x00, // movss [rsi+170],xmm0
    ],
    patch: Patch::Cave {
        overwrite_len: 12,
        shellcode: &[
            0xF3, 0x0F, 0x10, 0x86, 0x70, 0x01, 0x00, 0x00, // movss xmm0,[rsi+170]
            0xF3, 0x0F, 0x11, 0x00, // movss [rax],xmm0
            0xF3, 0x0F, 0x10, 0x00, // movss xmm0,[rax]
            0xF3, 0x0F, 0x11, 0x86, 0x70, 0x01, 0x00, 0x00, // movss [rsi+170],xmm0
        ],
    },
};

/// Automatic camera yaw adjustment on XY-axis movement. xmm0 holds the new yaw,
/// `[rsi+174]` the yaw before moving, so xmm0 is reloaded with the old yaw.
///
/// ```text
/// E8 B7BCFFFF        call ...
/// F3:0F1186 74010000 movss [rsi+174],xmm0   <- overwritten ; camYaw = newCamYaw
/// E9 9A020000        jmp ...                <- return here
/// ```
pub const CAMADJUST_YAW_XY: Site = Site {
    name: "camera yaw (XY)",
    pattern: pattern!("E8 ?? ?? ?? ?? F3 0F 11 86 ?? ?? 00 00 E9"),
    offset: 5,
    expected: &[0xF3, 0x0F, 0x11, 0x86, 0x74, 0x01, 0x00, 0x00], // movss [rsi+174],xmm0
    patch: Patch::Cave {
        overwrite_len: 8,
        shellcode: &[
            0xF3, 0x0F, 0x10, 0x86, 0x74, 0x01, 0x00, 0x00, // movss xmm0,[rsi+174]
            0xF3, 0x0F, 0x11, 0x86, 0x74, 0x01, 0x00, 0x00, // movss [rsi+174],xmm0
        ],
    },
};

// ---------------------------------------------------------------------------
// "Disable camera reset on lock-on"
// ---------------------------------------------------------------------------

/// Pressing lock-on with no target in range sets a flag that recenters the camera
/// and blocks camera input until that's done. The flag is now set to 0 instead of 1.
///
/// ```text
/// C686 A3020000 01   mov byte [rsi+2A3],1   ; the `01` is patched to `00`
/// F3:0F108E B4020000 movss xmm1,[rsi+2B4]
/// ```
pub const CAMRESET_LOCKON: Site = Site {
    name: "camera reset on lock-on",
    pattern: pattern!("C6 86 ?? ?? 00 00 ?? F3 0F 10 8E ?? ?? 00 00"),
    offset: 6,
    expected: &[0x01],
    patch: Patch::Bytes(&[0x00]),
};

// Checked at compile time: the bytes verified before patching lie inside the
// pattern match, cover everything that gets overwritten, and caves have room for a jmp.
const _: () = {
    let sites = [
        CAMADJUST_PITCH,
        CAMADJUST_YAW_Z,
        CAMADJUST_PITCH_XY,
        CAMADJUST_YAW_XY,
        CAMRESET_LOCKON,
    ];
    let mut i = 0;
    while i < sites.len() {
        let site = &sites[i];
        assert!(site.offset + site.expected.len() <= site.pattern.len());
        let overwritten = match site.patch {
            Patch::Bytes(bytes) => bytes.len(),
            Patch::Cave { overwrite_len, .. } => {
                assert!(overwrite_len >= 5);
                overwrite_len
            }
        };
        assert!(overwritten <= site.expected.len());
        i += 1;
    }
};
