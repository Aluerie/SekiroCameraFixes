//! Sekiro: Shadows Die Twice native mod, loaded by Mod Engine 3 (or any DLL loader).
//!
//! Reproduces two settings of uberhalit's "Sekiro FPS Unlocker and More":
//! "Disable camera auto rotate on movement" and "Disable camera reset on lock-on".

#[macro_use]
mod logfile;
#[macro_use]
mod pattern;
mod config;
mod error;
mod game_data;
mod memory;
mod threads;

use std::ffi::{OsString, c_void};
use std::io::ErrorKind;
use std::ops::Range;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{BOOL, HMODULE, MAX_PATH, TRUE};
use windows_sys::Win32::System::LibraryLoader::{DisableThreadLibraryCalls, GetModuleFileNameW};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

use config::Config;
use error::Error;
use game_data::{Patch, Site};
use logfile::LogFile;
use memory::{CavePage, Image};
use threads::Suspended;

const CONFIG_FILE: &str = "SekiroCameraFixes.ini";
const LOG_FILE: &str = "SekiroCameraFixes.log";

/// How long to keep looking for patterns, in case the game's code isn't fully in
/// place yet when the DLL is loaded.
const SEARCH_INTERVAL: Duration = Duration::from_millis(250);
const SEARCH_ATTEMPTS: u32 = 240;

/// This DLL's module handle, moved to the worker thread.
struct Module(HMODULE);

// SAFETY: a module handle is a process-wide value, valid from any thread.
unsafe impl Send for Module {}

/// DLL entry point.
///
/// # Safety
/// Only the Windows loader calls this, with a valid module handle.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    module: HMODULE,
    reason: u32,
    _reserved: *mut c_void,
) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        // SAFETY: `module` is this DLL's handle, given by the loader.
        unsafe { DisableThreadLibraryCalls(module) };
        let module = Module(module);
        // Patching happens off the loader lock. The thread starts running once
        // DllMain returns.
        let _ = thread::Builder::new()
            .name("SekiroCameraFixes".into())
            .spawn(move || run(module));
    }
    TRUE
}

fn run(module: Module) {
    let dll_dir = module_path(module.0)
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let mut log = LogFile::create(&dll_dir.join(LOG_FILE));
    log!(log, "SekiroCameraFixes {}", env!("CARGO_PKG_VERSION"));

    let exe = module_path(std::ptr::null_mut()).unwrap_or_default();
    let exe_name = exe.file_name().unwrap_or_default();
    if !exe_name.eq_ignore_ascii_case("sekiro.exe") {
        log!(
            log,
            "host process is {exe_name:?}, not sekiro.exe; doing nothing"
        );
        return;
    }

    let config = load_config(&dll_dir.join(CONFIG_FILE), &mut log);
    log!(log, "{config:?}");

    let mut pending = features(&config);
    if pending.is_empty() {
        log!(log, "all patches disabled in config");
        return;
    }

    let Some(image) = Image::main_module() else {
        log!(log, "could not read sekiro.exe image headers");
        return;
    };
    log!(
        log,
        "sekiro.exe image at {:p}, {} code section(s)",
        image.base(),
        image.code_sections()
    );

    for attempt in 1..=SEARCH_ATTEMPTS {
        let mut not_found = Vec::new();
        for feature in pending {
            match apply(&feature, &image) {
                Outcome::NotFound => not_found.push(feature),
                Outcome::Applied(addresses) => {
                    log!(log, "{}: applied", feature.name);
                    for (site, address) in feature.sites.iter().zip(addresses) {
                        log!(log, "  {} at {address:p}", site.name);
                    }
                }
                Outcome::Failed(error) => log!(log, "{}: NOT applied: {error}", feature.name),
            }
        }
        pending = not_found;
        if pending.is_empty() {
            return;
        }
        if attempt < SEARCH_ATTEMPTS {
            thread::sleep(SEARCH_INTERVAL);
        }
    }
    for feature in pending {
        log!(
            log,
            "{}: NOT applied: code not found (unsupported game version?)",
            feature.name
        );
    }
}

/// A user-facing setting and the sites it patches. All sites are applied, or none.
struct Feature {
    name: &'static str,
    sites: Vec<Site>,
}

fn features(config: &Config) -> Vec<Feature> {
    let mut features = Vec::new();
    if config.disable_auto_rotate {
        let mut sites = vec![game_data::CAMADJUST_PITCH, game_data::CAMADJUST_YAW_Z];
        if !config.controller {
            sites.push(game_data::CAMADJUST_PITCH_XY);
        }
        sites.push(game_data::CAMADJUST_YAW_XY);
        features.push(Feature {
            name: "Disable camera auto rotate on movement",
            sites,
        });
    }
    if config.disable_lockon_reset {
        features.push(Feature {
            name: "Disable camera reset on lock-on",
            sites: vec![game_data::CAMRESET_LOCKON],
        });
    }
    features
}

enum Outcome {
    /// At least one pattern isn't in memory (yet).
    NotFound,
    /// Patched; the address of each site.
    Applied(Vec<*mut u8>),
    /// Found but can't be patched safely; don't retry.
    Failed(Error),
}

/// Where a site's pattern is in the game's code.
enum Location {
    /// A match holding exactly the expected bytes; the patched address.
    Verified(*mut u8),
    /// The pattern matches, but no match holds the expected bytes. The address is
    /// the first match's patched address.
    Unexpected(*mut u8),
    /// The pattern doesn't match anywhere.
    Missing,
}

fn locate(site: &Site, image: &Image) -> Location {
    let expected = site.offset..site.offset + site.expected.len();
    let verified = image.find(site.pattern, |bytes| {
        bytes[expected.clone()] == *site.expected
    });
    if let Some(found) = verified {
        return Location::Verified(found.wrapping_add(site.offset));
    }
    match image.find(site.pattern, |_| true) {
        Some(found) => Location::Unexpected(found.wrapping_add(site.offset)),
        None => Location::Missing,
    }
}

fn apply(feature: &Feature, image: &Image) -> Outcome {
    // Locate every site before touching anything, so one site's patch can't
    // change where another site's pattern matches.
    let locations: Vec<Location> = feature
        .sites
        .iter()
        .map(|site| locate(site, image))
        .collect();
    if locations.iter().any(|l| matches!(l, Location::Missing)) {
        return Outcome::NotFound;
    }

    let mut addresses = Vec::with_capacity(locations.len());
    for (site, location) in feature.sites.iter().zip(locations) {
        match location {
            Location::Verified(address) => addresses.push(address),
            Location::Unexpected(address) => {
                return Outcome::Failed(Error::UnexpectedBytes {
                    site: site.name,
                    address,
                    found: memory::read(address, site.expected.len()).unwrap_or_default(),
                    expected: site.expected,
                });
            }
            Location::Missing => unreachable!("checked above"),
        }
    }

    match patch_sites(&feature.sites, &addresses) {
        Ok(()) => Outcome::Applied(addresses),
        Err(error) => Outcome::Failed(error),
    }
}

/// One site's new bytes and the game's bytes they replace.
struct Write {
    address: *mut u8,
    new: Vec<u8>,
    old: &'static [u8],
}

/// Patches every site, or none of them. `addresses` must hold each site's verified
/// expected bytes.
fn patch_sites(sites: &[Site], addresses: &[*mut u8]) -> Result<(), Error> {
    // Build every cave first; the game's code is only changed once all are ready.
    // If anything fails, dropping the page frees it.
    let mut page: Option<CavePage> = None;
    let mut writes = Vec::with_capacity(sites.len());
    for (site, &address) in sites.iter().zip(addresses) {
        let new = match site.patch {
            Patch::Bytes(bytes) => bytes.to_vec(),
            Patch::Cave {
                overwrite_len,
                shellcode,
            } => {
                let page = match &mut page {
                    Some(page) => page,
                    None => page.insert(CavePage::allocate(address)?),
                };
                let cave = page.add(shellcode, address.wrapping_add(overwrite_len))?;
                memory::hook_code(address.addr(), cave.addr(), overwrite_len)?
            }
        };
        // The overwritten bytes are within the expected ones (checked at compile time).
        let old = &site.expected[..new.len()];
        writes.push(Write { address, new, old });
    }
    if let Some(page) = &page {
        page.seal()?;
    }
    match write_all(&writes) {
        Ok(()) => {
            // The game's code now jumps into the page.
            if let Some(page) = page {
                page.keep();
            }
            Ok(())
        }
        // Every site holds its old bytes again, and the game's threads were
        // suspended the whole time, so nothing can be in the page; it is freed.
        Err((cause, false)) => Err(cause),
        Err((cause, true)) => {
            if let Some(page) = page {
                page.keep();
            }
            Err(Error::PartlyApplied {
                cause: Box::new(cause),
            })
        }
    }
}

/// How many times to wait for the game's threads to leave the code being patched.
const WRITE_ATTEMPTS: u32 = 1000;

/// Writes every site while the process's other threads are suspended, so none of
/// them runs half-written code. If a write fails, the sites already written are
/// restored. The error's flag is true if restoring failed too.
fn write_all(writes: &[Write]) -> Result<(), (Error, bool)> {
    // Each site except its first byte: a thread stopped there would resume in the
    // middle of the new instructions. One stopped on the first byte runs the new
    // code from its start, which is fine.
    let interiors: Vec<Range<usize>> = writes
        .iter()
        .map(|w| w.address.addr() + 1..w.address.addr() + w.new.len())
        .collect();
    for _ in 0..WRITE_ATTEMPTS {
        let threads = Suspended::others();
        if threads.any_inside(&interiors) {
            drop(threads);
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        // No heap allocation from here until `threads` is dropped.
        let result = write_or_restore(writes);
        drop(threads);
        return result;
    }
    Err((Error::CodeInUse, false))
}

/// Writes each site in order. On the first failure, puts back the sites already
/// written. Doesn't allocate.
fn write_or_restore(writes: &[Write]) -> Result<(), (Error, bool)> {
    for (i, write) in writes.iter().enumerate() {
        // SAFETY: the site holds `old`, the code `new` was written for, and no
        // thread is stopped inside it.
        if let Err(error) = unsafe { memory::write_code(write.address, &write.new) } {
            // A failed write_code changes nothing, so only earlier sites are restored.
            let mut restored = true;
            for done in &writes[..i] {
                // SAFETY: puts back the game's own bytes.
                restored &= unsafe { memory::write_code(done.address, done.old) }.is_ok();
            }
            return Err((error, !restored));
        }
    }
    Ok(())
}

fn load_config(path: &Path, log: &mut LogFile) -> Config {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let (config, warnings) = Config::parse(&text);
            for warning in warnings {
                log!(log, "{CONFIG_FILE}: {warning}");
            }
            config
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            match std::fs::write(path, config::DEFAULT_INI) {
                Ok(()) => log!(log, "created default {}", path.display()),
                Err(e) => log!(
                    log,
                    "no {CONFIG_FILE} and could not create one ({e}); using defaults"
                ),
            }
            Config::default()
        }
        // Leave the file alone: it may hold settings that just aren't UTF-8.
        Err(e) => {
            log!(
                log,
                "could not read {} ({e}); using defaults. Save it as UTF-8 to fix this",
                path.display()
            );
            Config::default()
        }
    }
}

/// Full path of a loaded module; a null handle means the process's `.exe`.
fn module_path(module: HMODULE) -> Option<PathBuf> {
    let mut buffer = vec![0u16; MAX_PATH as usize];
    loop {
        let capacity = u32::try_from(buffer.len()).ok()?;
        // SAFETY: the capacity passed matches the buffer's length.
        let len = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), capacity) } as usize;
        if len == 0 {
            return None;
        }
        if len < buffer.len() {
            return Some(OsString::from_wide(&buffer[..len]).into());
        }
        // Truncated: retry with a bigger buffer.
        buffer.resize(buffer.len() * 2, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a tiny function shaped like the game's yaw store through the real
    /// patching path and checks that the cave keeps the old yaw.
    #[test]
    fn yaw_cave_keeps_old_value() {
        #[rustfmt::skip]
        let mut function = vec![
            0x56,                   // push rsi
            0x48, 0x89, 0xCE,       // mov rsi,rcx
            0xF3, 0x0F, 0x10, 0x02, // movss xmm0,[rdx]
        ];
        let site_offset = function.len();
        function.extend_from_slice(game_data::CAMADJUST_YAW_Z.expected); // movss [rsi+174],xmm0
        function.extend_from_slice(&[0x5E, 0xC3]); // pop rsi; ret

        let near = (yaw_cave_keeps_old_value as *const ()).cast::<u8>();
        let mut code = CavePage::allocate(near).unwrap();
        let entry = code.add(&function, near).unwrap(); // trailing jmp is never reached
        code.seal().unwrap();
        // SAFETY: `entry` holds the win64 function assembled above.
        let call = unsafe {
            std::mem::transmute::<*const u8, extern "win64" fn(*mut u8, *const f32)>(entry)
        };

        let mut camera = [0u8; 0x200];
        let set_yaw = |camera: &mut [u8; 0x200], v: f32| {
            camera[0x174..0x178].copy_from_slice(&v.to_le_bytes())
        };
        let yaw =
            |camera: &[u8; 0x200]| f32::from_le_bytes(camera[0x174..0x178].try_into().unwrap());

        set_yaw(&mut camera, 1.0);
        call(camera.as_mut_ptr(), &2.0);
        assert_eq!(yaw(&camera), 2.0, "unpatched code stores the new yaw");

        let site = entry.cast_mut().wrapping_add(site_offset);
        patch_sites(&[game_data::CAMADJUST_YAW_Z], &[site]).unwrap();
        assert_eq!(memory::read(site, 1).unwrap(), [0xE9]);

        set_yaw(&mut camera, 1.0);
        call(camera.as_mut_ptr(), &2.0);
        assert_eq!(yaw(&camera), 1.0, "patched code keeps the old yaw");
    }

    /// When a later site can't be written, the sites already written get their
    /// original bytes back.
    #[test]
    fn failed_write_restores_earlier_sites() {
        let near = (failed_write_restores_earlier_sites as *const ()).cast::<u8>();
        let mut code = CavePage::allocate(near).unwrap();
        let site = code
            .add(game_data::CAMADJUST_YAW_Z.expected, near)
            .unwrap()
            .cast_mut();
        code.seal().unwrap();
        let before = memory::read(site, 16).unwrap();

        // The lowest 64 KiB of the address space are never mapped.
        let unmapped = std::ptr::without_provenance_mut(0x8000);
        let error = patch_sites(
            &[game_data::CAMADJUST_YAW_Z, game_data::CAMRESET_LOCKON],
            &[site, unmapped],
        )
        .unwrap_err();
        assert!(matches!(error, Error::Protect { .. }), "{error}");
        assert_eq!(memory::read(site, 16).unwrap(), before);
    }
}
