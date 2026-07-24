//! Table concurrency hammer — written for ThreadSanitizer runs (also a plain
//! stress test). Many OS threads race the lock-free dense path (put/get/has/
//! incr/delete) against a mid-run dense→hashed migration and a final drop,
//! exercising the MOVED-sentinel protocol on `table::Store` under a real data
//! race detector. Run under TSAN with:
//!
//!   RUSTFLAGS="-Zsanitizer=thread" cargo +nightly test -Zbuild-std \
//!       --target x86_64-unknown-linux-gnu -p brood --test table_tsan --release
//!
//! (Plain `cargo test` runs it too — it is a legitimate stress test either way.)

use brood::core::heap::Heap;
use brood::core::table;
use brood::core::value::Value;

fn heap() -> Heap {
    // A standalone heap per thread — table values are scalars here, so no GC
    // interplay; the table registry is process-global.
    Heap::new()
}

#[test]
fn dense_ops_race_migration_and_drop() {
    let id = table::create();
    let threads = 8;
    let per = 20_000;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(threads + 1));
    let mut joins = Vec::new();
    for t in 0..threads {
        let b = barrier.clone();
        joins.push(std::thread::spawn(move || {
            let mut h = heap();
            b.wait();
            for i in 0..per {
                let k = (t * per + i) as i64;
                table::put(&mut h, id, Value::int(k), Value::int(k * 3)).unwrap();
                table::incr(&mut h, id, Value::int(7_999_999), 1).unwrap();
                if i % 3 == 0 {
                    let got = table::get(&mut h, id, Value::int(k), Value::nil()).unwrap();
                    assert_eq!(got.as_int(), Some(k * 3), "read-your-write violated");
                }
                if i % 5 == 0 {
                    table::delete(&mut h, id, Value::int(k)).unwrap();
                    assert!(!table::has(&mut h, id, Value::int(k)).unwrap());
                    table::put(&mut h, id, Value::int(k), Value::int(k * 3)).unwrap();
                }
            }
        }));
    }
    // The migrating antagonist: flips the store to hashed mid-hammer.
    let b = barrier.clone();
    let mig = std::thread::spawn(move || {
        let mut h = heap();
        b.wait();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let key = h.alloc_string("force-migration");
        table::put(&mut h, id, key, Value::int(1)).unwrap();
    });
    for j in joins {
        j.join().unwrap();
    }
    mig.join().unwrap();
    // Exactness: every key present with its value; the shared counter exact.
    let mut h = heap();
    for t in 0..threads {
        for i in 0..per {
            let k = (t * per + i) as i64;
            assert_eq!(
                table::get(&mut h, id, Value::int(k), Value::nil())
                    .unwrap()
                    .as_int(),
                Some(k * 3)
            );
        }
    }
    assert_eq!(
        table::get(&mut h, id, Value::int(7_999_999), Value::nil())
            .unwrap()
            .as_int(),
        Some((threads * per) as i64)
    );
    // Drop under a fresh burst of readers: ops must error or succeed, never UB.
    let barrier2 = std::sync::Arc::new(std::sync::Barrier::new(3));
    let b1 = barrier2.clone();
    let r1 = std::thread::spawn(move || {
        let mut h = heap();
        b1.wait();
        for i in 0..10_000 {
            let _ = table::get(&mut h, id, Value::int(i), Value::nil());
        }
    });
    let b2 = barrier2.clone();
    let r2 = std::thread::spawn(move || {
        let mut h = heap();
        b2.wait();
        for i in 0..10_000 {
            let _ = table::put(&mut h, id, Value::int(i), Value::int(i));
        }
    });
    barrier2.wait();
    std::thread::sleep(std::time::Duration::from_millis(1));
    table::drop_table(id);
    r1.join().unwrap();
    r2.join().unwrap();
    let mut h = heap();
    assert!(table::get(&mut h, id, Value::int(1), Value::nil()).is_err());
}

#[test]
fn incr_only_exactness_across_migration() {
    let id = table::create();
    let threads = 8;
    let per = 30_000;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(threads));
    let mut joins = Vec::new();
    for t in 0..threads {
        let b = barrier.clone();
        joins.push(std::thread::spawn(move || {
            let mut h = heap();
            b.wait();
            for i in 0..per {
                table::incr(&mut h, id, Value::int(3), 1).unwrap();
                if t == 0 && i == per / 2 {
                    // migrate mid-storm from a hammering thread itself
                    let key = h.alloc_string("mig");
                    table::put(&mut h, id, key, Value::nil()).unwrap();
                }
            }
        }));
    }
    for j in joins {
        j.join().unwrap();
    }
    let mut h = heap();
    assert_eq!(
        table::get(&mut h, id, Value::int(3), Value::nil())
            .unwrap()
            .as_int(),
        Some((threads * per) as i64)
    );
}
