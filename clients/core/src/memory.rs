//! How much memory the app holds, and giving freed memory back.
//!
//! Measured on a real library (161k tracks, 2026-10-07, `catalog::measure`): the catalog
//! takes about 130 MB, and a refresh builds the new one while the old is still shown (about
//! 140 MB more for a moment). The system allocator (glibc) keeps what's freed for later
//! instead of giving it back, so each refresh would leave that much more held by the app,
//! adding up. `trim` gives it back; it's called after the catalog is replaced, when covers
//! are let go, and once a minute.

/// The app's resident memory now and at its highest, in bytes (Linux and Windows; None
/// elsewhere).
#[cfg(not(windows))]
pub fn resident() -> Option<(u64, u64)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb = |key: &str| -> Option<u64> { status.lines().find(|l| l.starts_with(key))?.split_whitespace().nth(1)?.parse().ok() };
    Some((kb("VmRSS:")? * 1024, kb("VmHWM:")? * 1024))
}

/// The working set now and at its highest.
#[cfg(windows)]
pub fn resident() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: the current process's pseudo-handle, and a buffer of the size said.
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) } != 0;
    ok.then_some((counters.WorkingSetSize as u64, counters.PeakWorkingSetSize as u64))
}

/// Gives memory the app freed back to the system (glibc's `malloc_trim`), on a thread of
/// its own: with a big heap it takes a few milliseconds.
pub fn trim() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    std::thread::spawn(|| {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        // SAFETY: glibc's own function, safe to call from any thread at any time.
        unsafe { malloc_trim(0) };
    });
}
