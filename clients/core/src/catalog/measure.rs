//! How much memory the catalog takes, measured by counting what's allocated (tests only):
//! `ACETATE_BENCH_CATALOG=<catalog.json> cargo test --release where_memory_goes -- --ignored --nocapture`.
//!
//! The counts are glibc's blocks (each request rounded up, plus its header), so they're
//! close to what the process holds. Other tests running at the same time add to them: run
//! this one alone.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// glibc's block for a request.
fn block(n: usize) -> usize {
    if n == 0 { 0 } else { (n + 8).div_ceil(16) * 16 }.max(32)
}

fn grew(n: usize) {
    let live = LIVE.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(live, Relaxed);
}

// SAFETY: hands every call to the system allocator unchanged; only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grew(block(layout.size()));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(block(layout.size()), Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        LIVE.fetch_sub(block(layout.size()), Relaxed);
        grew(block(size));
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

fn mb(n: usize) -> String {
    format!("{:.1} MB", n as f64 / 1e6)
}

/// Peak memory over `before` while `f` runs, and its result.
fn peak<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = LIVE.load(Relaxed);
    PEAK.store(before, Relaxed);
    let out = f();
    (out, PEAK.load(Relaxed) - before)
}

#[test]
#[ignore]
fn where_memory_goes() {
    use super::{index::Merger, model};
    let Ok(path) = std::env::var("ACETATE_BENCH_CATALOG") else { return };
    let body = std::fs::read(path).unwrap();
    let before = LIVE.load(Relaxed);
    let t = std::time::Instant::now();
    let (catalog, load_peak) = peak(|| Merger::from_base(model::parse_base(&body).unwrap()).unwrap().build(None).unwrap());
    let held = LIVE.load(Relaxed) - before;
    println!("{} tracks, {} albums, loaded in {:?}", catalog.track_count(), catalog.album_count(), t.elapsed());
    println!("catalog: {} (at most {} more while loading, the body's {} aside)", mb(held), mb(load_peak - held), mb(body.len()));
    for (part, bytes) in catalog.bytes() {
        println!("  {part}: {}", mb(bytes));
    }
    drop(body);

    // A delta of one track, as a refresh applies it: both catalogs are held for a moment.
    let mut changed = catalog.track_at(5000).to_model();
    changed.title.push_str(" (edited)");
    let delta = model::Raw {
        format: model::FORMAT,
        version: Some("1:2:".into()),
        delta: true,
        removed: model::Removed::default(),
        tracks: model::Part { columns: Vec::new(), rows: vec![changed] },
        albums: model::Part::default(),
        artists: model::Part::default(),
        annotations: model::Part::default(),
        issues: model::Part::default(),
        lyrics: model::Part::default(),
        groupings: model::Part::default(),
        extras: model::Part::default(),
        playlists: Vec::new(),
    };
    let t = std::time::Instant::now();
    let (applied, apply_peak) = peak(|| catalog.apply(delta));
    println!("a delta applied in {:?}: at most {} more while the old one is shown", t.elapsed(), mb(apply_peak));
    assert_eq!(applied.track_count(), catalog.track_count());
}
