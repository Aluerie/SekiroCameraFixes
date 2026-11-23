//! Reading and patching the game's own memory from inside the process.

use std::ffi::c_void;
use std::mem::{self, offset_of};
use std::ops::Range;
use std::ptr::{self, NonNull};

use windows_sys::Win32::System::Diagnostics::Debug::{
    FlushInstructionCache, IMAGE_NT_HEADERS64, IMAGE_SCN_MEM_EXECUTE, IMAGE_SECTION_HEADER,
    ReadProcessMemory,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_FREE, MEM_RELEASE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ,
    PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_NOACCESS, PAGE_PROTECTION_FLAGS, PAGE_READWRITE,
    VirtualAlloc, VirtualFree, VirtualProtect, VirtualQuery,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::SystemServices::{
    IMAGE_DOS_HEADER, IMAGE_DOS_SIGNATURE, IMAGE_NT_SIGNATURE,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::error::Error;
use crate::pattern::Pattern;

/// The main executable's mapped image and where its code is.
#[derive(Debug)]
pub struct Image {
    base: *mut u8,
    size: usize,
    /// Executable sections, as offsets from `base`, in address order.
    code: Vec<Range<usize>>,
}

impl Image {
    /// The image of the process's `.exe`, described by its PE headers.
    pub fn main_module() -> Option<Self> {
        // SAFETY: the main module's headers stay mapped for the life of the process.
        // Headers are read unaligned because nothing guarantees their alignment.
        unsafe {
            let base: *mut u8 = GetModuleHandleW(ptr::null()).cast();
            if base.is_null() {
                return None;
            }
            let dos = base.cast::<IMAGE_DOS_HEADER>().read_unaligned();
            if dos.e_magic != IMAGE_DOS_SIGNATURE {
                return None;
            }
            let nt_ptr = base.add(usize::try_from(dos.e_lfanew).ok()?);
            let nt = nt_ptr.cast::<IMAGE_NT_HEADERS64>().read_unaligned();
            if nt.Signature != IMAGE_NT_SIGNATURE {
                return None;
            }
            let sections = nt_ptr
                .add(offset_of!(IMAGE_NT_HEADERS64, OptionalHeader))
                .add(usize::from(nt.FileHeader.SizeOfOptionalHeader))
                .cast::<IMAGE_SECTION_HEADER>();
            let mut code: Vec<Range<usize>> = (0..usize::from(nt.FileHeader.NumberOfSections))
                .map(|i| sections.add(i).read_unaligned())
                .filter(|section| section.Characteristics & IMAGE_SCN_MEM_EXECUTE != 0)
                .map(|section| {
                    let start = section.VirtualAddress as usize;
                    start..start + section.Misc.VirtualSize as usize
                })
                .collect();
            code.sort_by_key(|range| range.start);
            Some(Self {
                base,
                size: nt.OptionalHeader.SizeOfImage as usize,
                code,
            })
        }
    }

    pub fn base(&self) -> *const u8 {
        self.base
    }

    pub fn code_sections(&self) -> usize {
        self.code.len()
    }

    /// Address of the first match of `pattern` in the image's executable sections
    /// for which `accept`, given the matched bytes, returns true.
    ///
    /// The code is copied out in chunks rather than read in place, because the
    /// game's anti-tamper code can rewrite it while it's being searched.
    pub fn find(&self, pattern: Pattern, mut accept: impl FnMut(&[u8]) -> bool) -> Option<*mut u8> {
        const CHUNK: usize = 1 << 20;
        // Consecutive chunks overlap so a match straddling their border is still seen.
        let overlap = pattern.len() - 1;
        let mut buffer = Vec::new();
        for section in &self.code {
            for run in self.readable_runs(section.clone()) {
                let mut start = run.start;
                while start < run.end {
                    buffer.resize((CHUNK + overlap).min(run.end - start), 0);
                    if read_into(self.base.wrapping_add(start), &mut buffer) {
                        for offset in pattern.matches(&buffer) {
                            if accept(&buffer[offset..offset + pattern.len()]) {
                                return Some(self.base.wrapping_add(start + offset));
                            }
                        }
                    }
                    start += CHUNK;
                }
            }
        }
        None
    }

    /// Contiguous readable parts of `section` (offsets from `base`), in address order.
    fn readable_runs(&self, section: Range<usize>) -> Vec<Range<usize>> {
        let end = section.end.min(self.size);
        let mut runs: Vec<Range<usize>> = Vec::new();
        let mut cursor = section.start;
        while cursor < end {
            let Some(info) = query(self.base.wrapping_add(cursor)) else {
                break;
            };
            let region_end =
                (info.BaseAddress.addr() + info.RegionSize - self.base.addr()).min(end);
            if region_end <= cursor {
                break;
            }
            if is_readable(&info) {
                match runs.last_mut() {
                    Some(run) if run.end == cursor => run.end = region_end,
                    _ => runs.push(cursor..region_end),
                }
            }
            cursor = region_end;
        }
        runs
    }
}

fn query(address: *const u8) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut info = mem::MaybeUninit::<MEMORY_BASIC_INFORMATION>::uninit();
    let size = mem::size_of::<MEMORY_BASIC_INFORMATION>();
    // SAFETY: VirtualQuery accepts any address and fully writes `info` on success.
    unsafe {
        (VirtualQuery(address.cast(), info.as_mut_ptr(), size) == size).then(|| info.assume_init())
    }
}

fn is_readable(info: &MEMORY_BASIC_INFORMATION) -> bool {
    info.State == MEM_COMMIT
        && info.Protect & (PAGE_NOACCESS | PAGE_GUARD) == 0
        && info.Protect != 0
}

/// Copies `len` bytes starting at `address` out of process memory, or `None` if
/// any of them can't be read.
pub fn read(address: *const u8, len: usize) -> Option<Vec<u8>> {
    let mut buffer = vec![0; len];
    read_into(address, &mut buffer).then_some(buffer)
}

/// Fills `buffer` from process memory starting at `address`. Returns false if any
/// of it can't be read.
fn read_into(address: *const u8, buffer: &mut [u8]) -> bool {
    let mut copied = 0;
    // SAFETY: ReadProcessMemory checks the source range itself and fails on
    // inaccessible memory; the destination is `buffer`, with its exact length.
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address.cast(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut copied,
        )
    };
    ok != 0 && copied == buffer.len()
}

/// Overwrites code at `address`, temporarily making it writable. Either every byte
/// is written or, on error, none is. Doesn't allocate, so it can run while other
/// threads are suspended.
///
/// The range may reach into at most one memory region past the one it starts in,
/// which any patch smaller than a page satisfies.
///
/// # Safety
/// `address..address + bytes.len()` must be mapped, and the new bytes must leave
/// the code valid for any thread that runs it.
pub unsafe fn write_code(address: *mut u8, bytes: &[u8]) -> Result<(), Error> {
    // Unprotect every region first, one guard each, so each region gets its own
    // protection back and a failure leaves the code untouched.
    let end = address.addr() + bytes.len();
    let mut guards: [Option<Unprotect>; 2] = [None, None];
    let mut cursor = address;
    for guard in &mut guards {
        if cursor.addr() >= end {
            break;
        }
        let info = query(cursor).ok_or(Error::Protect { address: cursor })?;
        let region_end = (info.BaseAddress.addr() + info.RegionSize).min(end);
        *guard = Some(Unprotect::new(
            cursor,
            region_end - cursor.addr(),
            PAGE_EXECUTE_READWRITE,
        )?);
        cursor = cursor.with_addr(region_end);
    }
    if cursor.addr() < end {
        return Err(Error::Protect { address: cursor });
    }
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), address, bytes.len());
        FlushInstructionCache(GetCurrentProcess(), address.cast(), bytes.len());
    }
    Ok(())
}

/// Changes memory protection and restores the old one when dropped.
struct Unprotect {
    address: *mut u8,
    len: usize,
    old: PAGE_PROTECTION_FLAGS,
}

impl Unprotect {
    fn new(address: *mut u8, len: usize, protection: PAGE_PROTECTION_FLAGS) -> Result<Self, Error> {
        let mut old = 0;
        // SAFETY: VirtualProtect validates the range itself and fails on unmapped memory.
        if unsafe { VirtualProtect(address.cast(), len, protection, &mut old) } == 0 {
            return Err(Error::Protect { address });
        }
        Ok(Self { address, len, old })
    }
}

impl Drop for Unprotect {
    fn drop(&mut self) {
        let mut ignored = 0;
        // SAFETY: restores the protection this guard changed on the same range.
        unsafe { VirtualProtect(self.address.cast(), self.len, self.old, &mut ignored) };
    }
}

/// Encodes `jmp rel32` placed at address `from` that lands on address `to`, if in range.
pub fn jmp_rel32(from: usize, to: usize) -> Option<[u8; 5]> {
    let next = i64::try_from(from).ok()?.checked_add(5)?;
    let displacement = i32::try_from(i64::try_from(to).ok()?.checked_sub(next)?).ok()?;
    let mut out = [0xE9, 0, 0, 0, 0];
    out[1..].copy_from_slice(&displacement.to_le_bytes());
    Some(out)
}

/// Machine code for a cave at address `cave`: `shellcode` followed by a jump to `return_to`.
pub fn cave_code(cave: usize, shellcode: &[u8], return_to: usize) -> Result<Vec<u8>, Error> {
    let jmp = jmp_rel32(cave + shellcode.len(), return_to).ok_or(Error::OutOfJumpRange)?;
    Ok([shellcode, &jmp].concat())
}

/// The bytes written over a hooked site: a jump to `cave`, padded with `nop`s to
/// `overwrite_len` bytes. `overwrite_len` must be at least 5.
pub fn hook_code(site: usize, cave: usize, overwrite_len: usize) -> Result<Vec<u8>, Error> {
    assert!(overwrite_len >= 5, "no room for a jmp");
    let mut code = jmp_rel32(site, cave).ok_or(Error::OutOfJumpRange)?.to_vec();
    code.resize(overwrite_len, 0x90);
    Ok(code)
}

/// One page of memory close enough to the game's code for `jmp rel32` both ways.
///
/// Freed when dropped, unless `keep` made it permanent.
pub struct CavePage {
    base: NonNull<u8>,
    size: usize,
    used: usize,
}

impl CavePage {
    /// Allocates a read/write page within jump range of `near`.
    pub fn allocate(near: *const u8) -> Result<Self, Error> {
        // Stay inside ±2 GiB with a margin, like the original tool.
        const RANGE: usize = 0x7000_0000;
        let mut info = mem::MaybeUninit::<SYSTEM_INFO>::uninit();
        // SAFETY: GetSystemInfo always fills the struct.
        let info = unsafe {
            GetSystemInfo(info.as_mut_ptr());
            info.assume_init()
        };
        let granularity = info.dwAllocationGranularity as usize;
        let size = info.dwPageSize as usize;
        let near = near.addr();
        let low = near
            .saturating_sub(RANGE)
            .max(info.lpMinimumApplicationAddress.addr());
        let high = near
            .saturating_add(RANGE)
            .min(info.lpMaximumApplicationAddress.addr());

        let mut cursor = low.next_multiple_of(granularity);
        while cursor < high {
            let info = query(ptr::without_provenance(cursor)).ok_or(Error::NoCaveMemory)?;
            let region_end = info.BaseAddress.addr() + info.RegionSize;
            if info.State == MEM_FREE {
                let start = info
                    .BaseAddress
                    .addr()
                    .max(cursor)
                    .next_multiple_of(granularity);
                if start + size <= region_end.min(high) {
                    // SAFETY: asks for a fresh allocation; the address is only a hint.
                    let got = unsafe {
                        VirtualAlloc(
                            ptr::without_provenance::<c_void>(start),
                            size,
                            MEM_RESERVE | MEM_COMMIT,
                            PAGE_READWRITE,
                        )
                    };
                    if let Some(base) = NonNull::new(got.cast::<u8>()) {
                        return Ok(Self {
                            base,
                            size,
                            used: 0,
                        });
                    }
                }
            }
            cursor = region_end.max(cursor + granularity);
        }
        Err(Error::NoCaveMemory)
    }

    /// Writes a cave that runs `shellcode` and jumps to `return_to`; returns its address.
    pub fn add(&mut self, shellcode: &[u8], return_to: *const u8) -> Result<*const u8, Error> {
        let offset = self.used.next_multiple_of(16);
        let at = self.base.as_ptr().wrapping_add(offset);
        let code = cave_code(at.addr(), shellcode, return_to.addr())?;
        if offset + code.len() > self.size {
            return Err(Error::CavePageFull);
        }
        // SAFETY: `at..at + code.len()` lies inside our own read/write allocation.
        unsafe { ptr::copy_nonoverlapping(code.as_ptr(), at, code.len()) };
        self.used = offset + code.len();
        Ok(at)
    }

    /// Makes the page executable and no longer writable.
    pub fn seal(&self) -> Result<(), Error> {
        let address = self.base.as_ptr();
        let mut old = 0;
        // SAFETY: the page is our own allocation.
        unsafe {
            if VirtualProtect(address.cast(), self.size, PAGE_EXECUTE_READ, &mut old) == 0 {
                return Err(Error::Protect { address });
            }
            FlushInstructionCache(GetCurrentProcess(), address.cast(), self.size);
        }
        Ok(())
    }

    /// Keeps the page for the rest of the process's life, because the game's code
    /// now jumps into it.
    pub fn keep(self) {
        mem::forget(self);
    }
}

impl Drop for CavePage {
    fn drop(&mut self) {
        // SAFETY: the page is our own allocation, and pages the game can jump into
        // are kept instead of dropped.
        unsafe { VirtualFree(self.base.as_ptr().cast(), 0, MEM_RELEASE) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jmp_forward_and_backward() {
        assert_eq!(jmp_rel32(0x1000, 0x1005), Some([0xE9, 0, 0, 0, 0]));
        assert_eq!(jmp_rel32(0x1000, 0x1105), Some([0xE9, 0x00, 0x01, 0, 0]));
        assert_eq!(
            jmp_rel32(0x1000, 0x1000),
            Some([0xE9, 0xFB, 0xFF, 0xFF, 0xFF])
        );
    }

    #[test]
    fn jmp_out_of_range() {
        assert_eq!(jmp_rel32(0x1_0000_0000, 0x3_0000_0000), None);
        assert_eq!(jmp_rel32(usize::MAX, 0), None);
    }

    #[test]
    fn hook_pads_with_nops() {
        let hook = hook_code(0x1000, 0x2000, 8).unwrap();
        assert_eq!(hook, [0xE9, 0xFB, 0x0F, 0, 0, 0x90, 0x90, 0x90]);
    }

    #[test]
    fn cave_jumps_back_after_shellcode() {
        let code = cave_code(0x2000, &[0x90, 0x90], 0x1008).unwrap();
        // jmp sits at 0x2002, ends at 0x2007; 0x1008 - 0x2007 = -0xFFF
        assert_eq!(code, [0x90, 0x90, 0xE9, 0x01, 0xF0, 0xFF, 0xFF]);
    }

    #[test]
    fn cave_page_round_trip() {
        let near = (cave_page_round_trip as *const ()).cast::<u8>();
        let mut page = CavePage::allocate(near).expect("allocation near test binary");
        assert!(jmp_rel32(near.addr(), page.base.addr().get()).is_some());
        let a = page.add(&[0xC3], near).unwrap();
        let b = page.add(&[0xC3], near).unwrap();
        assert_eq!(a.addr() % 16, 0);
        assert_eq!(b.addr(), a.addr() + 16);
        assert_eq!(
            read(a, 6).unwrap(),
            cave_code(a.addr(), &[0xC3], near.addr()).unwrap()
        );
        page.seal().unwrap();
        assert_eq!(query(a).unwrap().Protect, PAGE_EXECUTE_READ);
    }

    #[test]
    fn unsealed_page_is_freed() {
        let near = (unsealed_page_is_freed as *const ()).cast::<u8>();
        let page = CavePage::allocate(near).unwrap();
        let base = page.base.as_ptr();
        assert_eq!(query(base).unwrap().State, MEM_COMMIT);
        drop(page);
        assert_eq!(query(base).unwrap().State, MEM_FREE);
    }

    #[test]
    fn finds_own_code() {
        let image = Image::main_module().unwrap();
        assert!(image.code_sections() > 0);
        let pattern_of = |bytes: &[u8]| {
            Pattern::new(Box::leak(
                bytes.iter().map(|&b| Some(b)).collect::<Box<[_]>>(),
            ))
        };
        // Search for the first bytes of this function; any match must hold them.
        let function = (finds_own_code as *const ()).cast::<u8>();
        let bytes = read(function, 16).unwrap();
        let found = image
            .find(pattern_of(&bytes), |_| true)
            .expect("own code found");
        assert_eq!(read(found, 16).unwrap(), bytes);
        // `accept` can turn down every match.
        assert_eq!(image.find(pattern_of(&bytes), |_| false), None);
        // The headers aren't code, so they're not searched. Their first 128 bytes,
        // DOS stub message included, appear nowhere else.
        let header = read(image.base(), 128).unwrap();
        assert_eq!(image.find(pattern_of(&header), |_| true), None);
    }

    #[test]
    fn read_fails_on_unmapped_memory() {
        assert_eq!(read(ptr::null(), 1), None);
    }

    #[test]
    fn write_code_across_two_regions() {
        // Three pages: read/write, executable, read/write. Each is its own region.
        let near = (write_code_across_two_regions as *const ()).cast::<u8>();
        let page = CavePage::allocate(near).unwrap();
        let size = page.size;
        drop(page);
        let base: *mut u8 = unsafe {
            VirtualAlloc(
                ptr::null(),
                3 * size,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        }
        .cast();
        assert!(!base.is_null());
        let second = base.wrapping_add(size);
        let mut old = 0;
        assert_ne!(
            unsafe { VirtualProtect(second.cast(), size, PAGE_EXECUTE_READ, &mut old) },
            0
        );

        let start = second.wrapping_sub(2);
        unsafe { write_code(start, &[1, 2, 3, 4]).unwrap() };
        assert_eq!(read(start, 4).unwrap(), [1, 2, 3, 4]);
        assert_eq!(query(base).unwrap().Protect, PAGE_READWRITE);
        assert_eq!(query(second).unwrap().Protect, PAGE_EXECUTE_READ);

        // A range over three regions is refused without writing anything.
        let start = second.wrapping_sub(1);
        let before = read(start, size + 2).unwrap();
        assert!(unsafe { write_code(start, &vec![0xCC; size + 2]) }.is_err());
        assert_eq!(read(start, size + 2).unwrap(), before);
        assert_eq!(query(second).unwrap().Protect, PAGE_EXECUTE_READ);

        unsafe { VirtualFree(base.cast(), 0, MEM_RELEASE) };
    }
}
