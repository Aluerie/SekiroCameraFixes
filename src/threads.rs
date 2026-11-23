//! Pausing the process's other threads while the game's code is rewritten.

use std::ops::Range;
use std::ptr;
use std::sync::{Mutex, MutexGuard};

use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CONTEXT, CONTEXT_CONTROL_AMD64, GetThreadContext,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread,
    THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
};

/// Every other thread of the process, suspended until this is dropped.
///
/// While it exists, the current thread must not allocate or free heap memory: a
/// suspended thread may be holding the heap's lock, and waiting for it would hang
/// the game. Threads started after the thread list was taken are not suspended.
pub struct Suspended {
    /// Null for threads that couldn't be suspended.
    handles: Vec<HANDLE>,
    /// Two threads suspending each other at once would both stop for good, so only
    /// one `Suspended` exists at a time. Released after the threads are resumed.
    _exclusive: MutexGuard<'static, ()>,
}

static EXCLUSIVE: Mutex<()> = Mutex::new(());

impl Suspended {
    pub fn others() -> Self {
        let exclusive = EXCLUSIVE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Open every handle before suspending anything, because building the list
        // allocates.
        let mut handles: Vec<HANDLE> = other_thread_ids()
            .into_iter()
            .map(|id| {
                // SAFETY: OpenThread returns null for an id that no longer exists.
                unsafe {
                    OpenThread(
                        THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                        FALSE,
                        id,
                    )
                }
            })
            .filter(|handle| !handle.is_null())
            .collect();
        for handle in &mut handles {
            // SAFETY: `handle` is an open thread handle with suspend access.
            if unsafe { SuspendThread(*handle) } == u32::MAX {
                // SAFETY: closes the handle opened above, which isn't used again.
                unsafe { CloseHandle(*handle) };
                *handle = ptr::null_mut();
            }
        }
        Self {
            handles,
            _exclusive: exclusive,
        }
    }

    /// Whether any suspended thread's next instruction is inside one of `ranges`.
    /// A thread whose state can't be read counts as outside.
    pub fn any_inside(&self, ranges: &[Range<usize>]) -> bool {
        self.handles
            .iter()
            .filter(|handle| !handle.is_null())
            .filter_map(|&handle| instruction_pointer(handle))
            .any(|rip| ranges.iter().any(|range| range.contains(&rip)))
    }
}

impl Drop for Suspended {
    fn drop(&mut self) {
        for &handle in self.handles.iter().filter(|handle| !handle.is_null()) {
            // SAFETY: each handle was suspended by `others` and is closed only here.
            unsafe {
                ResumeThread(handle);
                CloseHandle(handle);
            }
        }
    }
}

/// `CONTEXT` with the 16-byte alignment `GetThreadContext` requires.
#[repr(C, align(16))]
struct AlignedContext(CONTEXT);

/// Address of the next instruction a suspended thread will run.
fn instruction_pointer(thread: HANDLE) -> Option<usize> {
    // SAFETY: CONTEXT is plain data, for which all zeroes is a valid value.
    let mut context: AlignedContext = unsafe { std::mem::zeroed() };
    context.0.ContextFlags = CONTEXT_CONTROL_AMD64;
    // SAFETY: `thread` has get-context access and `context` is suitably aligned.
    // For a suspended thread this also waits until it has really stopped.
    let ok = unsafe { GetThreadContext(thread, &mut context.0) };
    (ok != 0).then_some(context.0.Rip as usize)
}

/// Ids of all threads in this process except the current one.
fn other_thread_ids() -> Vec<u32> {
    let mut ids = Vec::new();
    // SAFETY: plain Toolhelp calls on a snapshot this function owns and closes.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return ids;
        }
        let process = GetCurrentProcessId();
        let current = GetCurrentThreadId();
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut more = Thread32First(snapshot, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == process && entry.th32ThreadID != current {
                ids.push(entry.th32ThreadID);
            }
            more = Thread32Next(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn suspends_and_resumes_other_threads() {
        let counter = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let started = Arc::new(Barrier::new(2));
        let worker = {
            let (counter, stop, started) = (counter.clone(), stop.clone(), started.clone());
            thread::spawn(move || {
                started.wait();
                while !stop.load(Ordering::Relaxed) {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            })
        };
        started.wait();

        let suspended = Suspended::others();
        // Empty ranges contain nothing; the whole address space contains every thread.
        assert!(!suspended.any_inside(&[0..0, 0..0]));
        assert!(suspended.any_inside(&[0..0, 0..usize::MAX]));
        let before = counter.load(Ordering::Relaxed);
        std::hint::spin_loop();
        assert_eq!(
            counter.load(Ordering::Relaxed),
            before,
            "worker kept running"
        );
        drop(suspended);

        while counter.load(Ordering::Relaxed) == before {
            std::hint::spin_loop();
        }
        stop.store(true, Ordering::Relaxed);
        worker.join().unwrap();
    }
}
