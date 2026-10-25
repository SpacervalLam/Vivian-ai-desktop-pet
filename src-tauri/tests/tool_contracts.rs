//! Exercise the production tool infrastructure without a desktop or user data.
#![allow(dead_code)]

#[path = "../src/tools/arguments.rs"]
pub mod arguments;
#[path = "../src/tools/cache.rs"]
pub mod cache;
#[path = "../src/tools/catalog.rs"]
pub mod catalog;
#[path = "../src/tools/observability.rs"]
pub mod observability;
#[path = "../src/tools/terminal_buffer.rs"]
pub mod terminal_buffer;

#[cfg(test)]
mod benchmarks {
    use super::cache::ToolCache;
    use serde_json::json;
    use std::{hint::black_box, time::Instant};

    #[test]
    #[ignore = "manual before/after measurement"]
    fn cache_hit_scaling() {
        for capacity in [1_000, 10_000] {
            let cache = ToolCache::new(3600, capacity);
            for n in 0..capacity {
                cache.set("probe", &json!({"n": n}), json!({"value": n}));
            }
            let args = json!({"n": capacity / 2});
            for _ in 0..100 {
                black_box(cache.get("probe", &args));
            }
            let samples: Vec<_> = (0..5)
                .map(|_| {
                    let start = Instant::now();
                    for _ in 0..30_000 {
                        black_box(cache.get("probe", &args));
                    }
                    start.elapsed().as_micros()
                })
                .collect();
            eprintln!("capacity={capacity}, 30000 warm hits (us): {samples:?}");
            assert_eq!(cache.stats()["total_entries"], capacity);
        }
    }
}
