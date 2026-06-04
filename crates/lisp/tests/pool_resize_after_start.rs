//! `set_max_parallel` after the worker pool has started must be a no-op, not a
//! panic (kernel audit, latent finding). The pool is sized once at the first
//! spawn; before the fix, `assign_worker` re-derived its modulus from
//! `worker_count()` on every spawn — so a *later* `set_max_parallel(huge)`
//! made the rotating scan index `WORKERS[i]` past the committed pool's length
//! (an OOB panic on the spawn path). Now everything indexes by `WORKERS.len()`.
//!
//! Its own test binary: `set_max_parallel` is process-wide, so the oversized
//! value must not leak into the other test binaries.

use brood::{process, Interp};

#[test]
fn set_max_parallel_after_pool_start_is_inert() {
    let mut interp = Interp::new();
    // First spawn commits the pool at the default size (≈ nproc).
    let v = interp
        .eval_str(
            r#"
            (def me (self))
            (spawn (send me :first))
            (receive (:first :ok) (after 3000 :timeout))
            "#,
        )
        .expect("first spawn errored");
    assert_eq!(interp.print(v), ":ok");

    // Way past any real core count — before the fix the next spawns' rotating
    // least-loaded scan computed indices modulo THIS and panicked OOB.
    process::set_max_parallel(4096);

    let v = interp
        .eval_str(
            r#"
            (def me (self))
            (defn fan (n)
              (if (= n 0)
                  nil
                  (do (spawn (send me :hi)) (fan (- n 1)))))
            (defn drain (n)
              (if (= n 0)
                  :all-arrived
                  (receive (:hi (drain (- n 1))) (after 3000 :timeout))))
            (fan 64)
            (drain 64)
            "#,
        )
        .expect("post-resize spawns errored");
    assert_eq!(
        interp.print(v),
        ":all-arrived",
        "spawning after a late set_max_parallel must still schedule on the committed pool"
    );
}
