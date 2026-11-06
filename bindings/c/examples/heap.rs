//! Heap the engine holds for one delivery: peak while loading and playing, and what stays.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
struct Counting;
static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let n = NOW.fetch_add(l.size(), Relaxed) + l.size();
        PEAK.fetch_max(n, Relaxed);
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        NOW.fetch_sub(l.size(), Relaxed);
        System.dealloc(p, l)
    }
}
#[global_allocator]
static A: Counting = Counting;
fn main() {
    let json = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let base = NOW.load(Relaxed);
    PEAK.store(base, Relaxed);
    let mut e = masko_player_engine::load(&json).unwrap();
    let after_load = NOW.load(Relaxed) - base;
    let mut t = 0u64;
    let _ = e.start(t);
    for name in ["behavior::working", "behavior::attention", "behavior::rest"] {
        t += 5000;
        let _ = e.set(name, masko_player_engine::Value::Bool(true), false, t);
        for _ in 0..20 {
            t += 1000;
            let _ = e.advance(t);
        }
    }
    println!(
        "json {} KB | held after load {} KB | held after playing {} KB | peak {} KB",
        json.len() / 1024,
        after_load / 1024,
        (NOW.load(Relaxed) - base) / 1024,
        (PEAK.load(Relaxed) - base) / 1024
    );
}
