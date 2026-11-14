//! Generation-handle and flip tests: a stale handle trips the tripwire, a flushed root
//! stays valid, and every nursery kind that points at an old object is rewritten.

use super::*;
use crate::core::value::Value;

/// The generational-handle tripwire fires at the bad deref. A LOCAL handle
/// held across a collection without being passed through as a root carries a
/// stale generation epoch; dereferencing it must panic *here* with a
/// "use-after-GC" message — not a far-away out-of-bounds index. Debug-only
/// check; `cargo test` builds with `debug_assertions` on.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "use-after-GC")]
fn stale_handle_after_collect_panics() {
    let mut h = Heap::new();
    h.gc_enabled = true; // `Heap::new` is a builder heap, where `collect` is gated off
    let id = match h.alloc_pair(Value::int(1), Value::int(2)).unpack() {
        ValueRef::Pair(id) => id,
        _ => unreachable!(),
    };
    // Collect with no roots: the pair isn't relocated, and the epoch bumps.
    h.collect(&mut [], &mut []);
    // `id` was minted in the previous epoch → stale → tripwire.
    let _ = h.pair(id);
}

/// The mirror case: a handle passed through `collect` as a root is relocated
/// and re-stamped with the new epoch, so it stays valid (no false positive).
#[test]
fn collected_root_handle_stays_valid() {
    let mut h = Heap::new();
    h.gc_enabled = true;
    let mut roots = [h.alloc_pair(Value::int(1), Value::int(2))];
    let runs_before = h.gc_runs;
    h.collect(&mut roots, &mut []);
    assert_eq!(h.gc_runs, runs_before + 1, "the collection did not run");
    let (car, _) = match roots[0].unpack() {
        ValueRef::Pair(id) => h.pair(id),
        _ => unreachable!(),
    };
    assert!(matches!(car.unpack(), ValueRef::Int(1)));
}

/// `reset_local_to` truncates only the NURSERY, so it must drop only the nursery
/// form positions past the checkpoint. A tenured pair's position key carries the
/// age bit (`form_pos_key`); comparing its bare low-32 index against the
/// checkpoint's nursery length dropped every OLD form's position at index ≥ the
/// nursery checkpoint — every one, right after a tenure empties the nursery.
#[test]
fn reset_local_to_keeps_old_form_positions() {
    use crate::error::Pos;
    let mut h = Heap::new();
    let old_pos = Pos { line: 4, col: 2 };
    let young_pos = Pos { line: 8, col: 1 };
    let old_form = h.alloc_pair(Value::int(1), Value::nil());
    h.set_form_pos(old_form, old_pos);
    let mut roots = vec![old_form];
    h.minor_collect(true, &mut roots, &mut []); // tenure: the nursery is now empty
    let old_form = roots[0];
    assert!(matches!(old_form.unpack(), ValueRef::Pair(id) if id.is_old()));

    let checkpoint = h.checkpoint();
    let young_form = h.alloc_pair(Value::int(2), Value::nil());
    h.set_form_pos(young_form, young_pos);
    assert_eq!(h.form_pos_only(young_form), Some(young_pos));
    h.reset_local_to(checkpoint);

    assert_eq!(
        h.form_pos_only(old_form),
        Some(old_pos),
        "reset_local_to dropped the position of a tenured form it never truncated",
    );
    // The reclaimed nursery pair's entry is gone: a fresh pair reusing its slot
    // has no position.
    let reused = h.alloc_pair(Value::int(3), Value::nil());
    assert_eq!(h.form_pos_only(reused), None);
}

/// Regression (`docs/kernel-audit-2026-06-03.md` #1): a **major collection
/// that follows a *flip* minor** must rewrite the write-barrier `remembered`
/// set through the env forwarding table. A flip *retains* `remembered` (the
/// old->young edges persist); the subsequent major then relocates those old
/// frames and bumps `old_epoch`, leaving every retained entry a stale index
/// *and* a stale epoch. The next minor indexes `self.old().envs[e.index()]`
/// directly — no bounds/epoch check — so a stale entry is a silent
/// use-after-GC: an OOB `Vec` panic when the compacted old gen is smaller, a
/// wrong-frame read otherwise. (`BROOD_GC_VERIFY` misses it too — its
/// remembered walk uses a safe `.get()`.)
///
/// We drive the full `tenure -> mid-bind env_define -> flip -> major -> minor`
/// interleave with the remembered frame at a HIGH old-gen index that the major
/// then compacts away, so a stale index would be out of bounds. Without the
/// fix the post-major assertions fail (stale epoch / OOB index); with it they
/// hold and the trailing minor derefs the rewritten handle cleanly.
#[test]
fn major_after_flip_rewrites_remembered_set() {
    let mut h = Heap::new();
    let x = crate::core::value::intern("x");
    // Inflate the nursery with several frames, all rooted; the one we keep is
    // LAST so it tenures to the highest old-gen index.
    let mut envs = Vec::from_fn(8, |_| h.new_env(None));
    let keep_pos = envs.len() - 1;
    // Tenure them all into the old generation.
    h.minor_collect(true, &mut [], &mut envs);
    let keep = envs[keep_pos];
    assert!(keep.is_old(), "frame should be tenured into the old gen");
    let high_index = keep.index();
    assert!(
        high_index > 0,
        "kept frame should sit at a non-zero old index"
    );
    // Mid-bind define: store a *young* value into the tenured frame, recording
    // an old->young edge in `remembered`.
    let young = h.alloc_pair(Value::int(1), Value::int(2));
    h.env_define(keep, x, young);
    assert_eq!(
        h.remembered.len(),
        1,
        "the old->young edge should be remembered"
    );
    // Keep only `keep` rooted, so the upcoming major reclaims the 7 inflation
    // frames and *shrinks* the old gen below `high_index`.
    let mut roots = vec![keep];
    // A *flip* minor (tenure=false): retains `remembered`, leaves old untouched.
    h.minor_collect(false, &mut [], &mut roots);
    assert_eq!(h.remembered.len(), 1, "a flip must retain remembered");
    // The major: compacts old (drops the 7 dead frames), bumps `old_epoch`.
    h.major_collect(&mut [], &mut roots);
    let keep = roots[0];
    // With the fix, `remembered` was rewritten through the forwarding table:
    // current epoch, in bounds, pointing at the surviving frame. Without it,
    // these carry the stale pre-major index/epoch.
    assert_eq!(h.remembered.len(), 1);
    let r = h.remembered[0];
    assert!(r.is_old());
    assert_eq!(
        r.generation(),
        h.old_epoch,
        "remembered entry kept a stale epoch after the major (use-after-GC)"
    );
    assert!(
        r.index() < h.old().envs.len(),
        "remembered entry is out of bounds after the major (use-after-GC): \
         index {} >= old.envs.len() {}",
        r.index(),
        h.old().envs.len(),
    );
    assert_eq!(
        r.index(),
        keep.index(),
        "remembered should point at the surviving kept frame"
    );
    // The deref path the bug corrupts: a subsequent minor reads
    // `self.old().envs[r.index()]`. Must not panic; the young binding survives.
    h.minor_collect(false, &mut [], &mut roots);
    let keep = roots[0];
    assert!(keep.is_old());
    let bound = h.old().envs[keep.index()]
        .vars
        .iter()
        .find(|(s, _)| *s == x)
        .map(|(_, v)| *v);
    assert!(
        matches!(bound.map(Value::unpack), Some(ValueRef::Pair(_))),
        "the remembered young binding was lost or corrupted: {bound:?}"
    );
}

/// Repeated `env_define`s into the *same* tenured frame must not grow the
/// write-barrier `remembered` set (kernel audit, perf #3): one entry per
/// distinct old frame is all the minor's rewrite walk needs, and without
/// the de-dup a long binding loop on a tenured frame grows the set — and
/// every subsequent minor's walk — without bound until the next tenure.
#[test]
fn remembered_set_dedups_repeated_binds() {
    let mut h = Heap::new();
    let mut envs: Vec<EnvId> = vec![h.new_env(None)];
    h.minor_collect(true, &mut [], &mut envs);
    let frame = envs[0];
    assert!(frame.is_old(), "frame should be tenured into the old gen");
    for i in 0..64 {
        let sym = crate::core::value::intern(&format!("x{i}"));
        let young = h.alloc_pair(Value::int(i), Value::int(i));
        h.env_define(frame, sym, young);
    }
    assert_eq!(
        h.remembered.len(),
        1,
        "64 binds into one tenured frame must remember it once, not 64 times"
    );
    // The single entry still carries every young edge through a minor.
    let mut roots = vec![frame];
    h.minor_collect(false, &mut [], &mut roots);
    let frame = roots[0];
    assert_eq!(h.old().envs[frame.index()].vars.len(), 64);
}

// ── flush_nursery_old_refs regression tests ──────────────────────────────
//
// Pattern common to every test below:
//   1. Inflate old space with N decoy objects + 1 "keeper" (tenured together,
//      keeper is last → highest old index).
//   2. Drop decoy roots; keeper is now only referenced from a NURSERY object.
//   3. Flip minor: nursery object survives; decoys remain unreachable in old.
//   4. Major: old compacts. Decoys are freed; keeper moves to index 0.
//      Without the fix, the nursery object still holds the pre-major index
//      (e.g., 20) which is now out-of-bounds (len=1) — silent use-after-GC.
//      With the fix, flush_nursery_old_refs rewrites it to index 0.
//   5. Assert the nursery object's inner handle is in-bounds and reads correctly.

/// Tenure N strings + a keeper; drop decoys; return (inflated old space
/// keeper Value).  The keeper is pushed last so it sits at the highest index.
fn inflate_old_with_keeper_string(h: &mut Heap, n: usize, content: &str) -> Value {
    let mut roots: Vec<Value> = (0..n)
        .map(|i| h.alloc_string(&format!("decoy-{i}")))
        .collect();
    roots.push(h.alloc_string(content));
    h.minor_collect(true, &mut roots, &mut []);
    // Return only the keeper (last); caller holds no decoy roots.
    roots.pop().unwrap()
}

/// Nursery env frame with an OLD string var is rewritten after flip+major.
/// Hatch crash pattern: session env frame held a stale old StrId → OOB.
#[test]
fn nursery_env_old_string_var_rewritten() {
    let mut h = Heap::new();
    let sym = crate::core::value::intern("s");
    let keeper = inflate_old_with_keeper_string(&mut h, 20, "hello-env-var");
    let pre_major_idx = match keeper.unpack() {
        ValueRef::Str(id) => {
            assert!(id.is_old());
            id.index()
        }
        _ => unreachable!(),
    };

    let nursery_env = h.new_env(None);
    h.env_define(nursery_env, sym, keeper);
    // Only the nursery env keeps the keeper alive.
    let mut er = vec![nursery_env];
    h.minor_collect(false, &mut [], &mut er); // flip
    let _nursery_env = er[0];
    h.major_collect(&mut [], &mut er); // compact — decoys freed, keeper moves
    let nursery_env = er[0];

    let bound = h.local.envs[nursery_env.index()]
        .vars
        .iter()
        .find(|(s, _)| *s == sym)
        .map(|(_, v)| *v)
        .expect("binding must survive");
    let new_idx = match bound.unpack() {
        ValueRef::Str(id) => {
            assert!(id.is_old());
            id.index()
        }
        _ => panic!("expected Str"),
    };
    assert!(
        new_idx < h.old().strings.len(),
        "string index {new_idx} OOB (len {}); pre-major index was {pre_major_idx}",
        h.old().strings.len()
    );
    assert_eq!(h.old().strings[new_idx].as_str(), "hello-env-var");
}

/// Nursery env frame with an OLD pair var is rewritten after flip+major.
#[test]
fn nursery_env_old_pair_var_rewritten() {
    let mut h = Heap::new();
    let sym = crate::core::value::intern("p");
    // Inflate: 20 decoy pairs + keeper pair (1, 2).
    let mut roots: Vec<Value> = (0..20)
        .map(|i| h.alloc_pair(Value::int(i), Value::int(i)))
        .collect();
    roots.push(h.alloc_pair(Value::int(99), Value::int(100)));
    h.minor_collect(true, &mut roots, &mut []);
    let keeper_pair = roots.pop().unwrap();
    let pre_major_idx = match keeper_pair.unpack() {
        ValueRef::Pair(id) => {
            assert!(id.is_old());
            id.index()
        }
        _ => unreachable!(),
    };
    drop(roots);

    let nursery_env = h.new_env(None);
    h.env_define(nursery_env, sym, keeper_pair);
    let mut er = vec![nursery_env];
    h.minor_collect(false, &mut [], &mut er);
    let _nursery_env = er[0];
    h.major_collect(&mut [], &mut er);
    let nursery_env = er[0];

    let bound = h.local.envs[nursery_env.index()]
        .vars
        .iter()
        .find(|(s, _)| *s == sym)
        .map(|(_, v)| *v)
        .unwrap();
    let new_idx = match bound.unpack() {
        ValueRef::Pair(id) => {
            assert!(id.is_old());
            id.index()
        }
        _ => panic!("expected Pair"),
    };
    assert!(
        new_idx < h.old().pairs.len(),
        "pair index {new_idx} OOB (len {}); pre-major was {pre_major_idx}",
        h.old().pairs.len()
    );
    let (car, _) = h.old().pairs[new_idx];
    assert!(matches!(car.unpack(), ValueRef::Int(99)));
}

/// Nursery env frame with an OLD closure var is rewritten after flip+major.
#[test]
fn nursery_env_old_closure_var_rewritten() {
    use crate::core::value::Closure;
    let mut h = Heap::new();
    let sym = crate::core::value::intern("f");
    // Build a tiny closure and inflate old space around it.
    let cl = Closure::single(None, vec![], vec![], None, vec![], None, None);
    let cl_id = h.alloc_closure(cl);
    let cl_val = Value::func(cl_id);
    // 20 decoy closures to push keeper to a high index.
    let mut roots: Vec<Value> = (0..20)
        .map(|_| {
            let dc = Closure::single(None, vec![], vec![], None, vec![], None, None);
            Value::func(h.alloc_closure(dc))
        })
        .collect();
    roots.push(cl_val);
    h.minor_collect(true, &mut roots, &mut []);
    let keeper_fn = roots.pop().unwrap();
    let pre_major_idx = match keeper_fn.unpack() {
        ValueRef::Fn(id) => {
            assert!(id.is_old());
            id.index()
        }
        _ => unreachable!(),
    };
    drop(roots);

    let nursery_env = h.new_env(None);
    h.env_define(nursery_env, sym, keeper_fn);
    let mut er = vec![nursery_env];
    h.minor_collect(false, &mut [], &mut er);
    let _nursery_env = er[0];
    h.major_collect(&mut [], &mut er);
    let nursery_env = er[0];

    let bound = h.local.envs[nursery_env.index()]
        .vars
        .iter()
        .find(|(s, _)| *s == sym)
        .map(|(_, v)| *v)
        .unwrap();
    let new_idx = match bound.unpack() {
        ValueRef::Fn(id) => {
            assert!(id.is_old());
            id.index()
        }
        _ => panic!("expected Fn"),
    };
    assert!(
        new_idx < h.old().closures.len(),
        "closure index {new_idx} OOB (len {}); pre-major was {pre_major_idx}",
        h.old().closures.len()
    );
}

/// Nursery closure whose `env` field points to an OLD env is rewritten.
/// Pong crash pattern: nursery badge-ops closure had stale OLD env → wrong
/// env frame looked up → captured `throb` returned nil.
#[test]
fn nursery_closure_old_env_rewritten() {
    use crate::core::value::Closure;
    let mut h = Heap::new();
    let sym = crate::core::value::intern("throb");
    // Build the "token-ops" env that captures throb=3.14.
    // Inflate: 20 decoy envs + the keeper env (all tenured).
    let mut decoy_envs = Vec::from_fn(20, |_| h.new_env(None));
    let keeper_env = h.new_env(None);
    h.env_define(keeper_env, sym, Value::float(1.25));
    decoy_envs.push(keeper_env);
    h.minor_collect(true, &mut [], &mut decoy_envs);
    let keeper_env_old = decoy_envs.pop().unwrap();
    assert!(keeper_env_old.is_old());
    let pre_major_env_idx = keeper_env_old.index();
    drop(decoy_envs); // decoys now unreachable

    // Create a nursery closure that captures the OLD env (the "badge-ops" closure).
    let cl = Closure::single(
        None,
        vec![],
        vec![],
        None,
        vec![],
        None,
        Some(keeper_env_old),
    );
    let cl_id = h.alloc_closure(cl);
    assert!(!cl_id.is_old(), "closure must be nursery");

    let mut roots = [Value::func(cl_id)];
    h.minor_collect(false, &mut roots, &mut []); // flip
    h.major_collect(&mut roots, &mut []); // compact — 20 decoy envs freed

    let new_cl_id = match roots[0].unpack() {
        ValueRef::Fn(id) => id,
        _ => unreachable!(),
    };
    let env_id = h.local.closures[new_cl_id.index()]
        .env
        .expect("closure must have an env");

    assert!(
        env_id.is_old(),
        "closure env must still be old-gen after major"
    );
    assert!(
        env_id.index() < h.old().envs.len(),
        "closure env index {} OOB (len {}); pre-major was {pre_major_env_idx}",
        env_id.index(),
        h.old().envs.len()
    );
    // The env lookup chain must work: env_get returns throb = 3.14.
    let val = h.env_get(env_id, sym).expect("throb must be findable");
    assert!(
        matches!(val.unpack(), ValueRef::Float(f) if (f - 1.25).abs() < 1e-9),
        "throb should be 3.14, got {val:?}"
    );
}

/// Nursery closure whose per-arm optional-default Value is an OLD string.
#[test]
fn nursery_closure_old_optional_default_rewritten() {
    use crate::core::value::{Closure, ClosureArm};
    let mut h = Heap::new();
    let keeper = inflate_old_with_keeper_string(&mut h, 20, "default-str");
    let pre_idx = match keeper.unpack() {
        ValueRef::Str(id) => id.index(),
        _ => unreachable!(),
    };

    let opt_sym = crate::core::value::intern("opt");
    let cl = Closure {
        name: None,
        arms: vec![ClosureArm {
            params: vec![],
            optionals: vec![(opt_sym, keeper)],
            rest: None,
            body: vec![],
            passthrough: None,
        }]
        .into(),
        doc: None,
        env: None,
        module: None,
        identity: crate::core::value::fresh_closure_identity(),
    };
    let cl_id = h.alloc_closure(cl);
    let mut roots = [Value::func(cl_id)];
    h.minor_collect(false, &mut roots, &mut []);
    h.major_collect(&mut roots, &mut []);

    let new_cl_id = match roots[0].unpack() {
        ValueRef::Fn(id) => id,
        _ => unreachable!(),
    };
    let opt_val = h.local.closures[new_cl_id.index()].arms[0].optionals[0].1;
    let new_idx = match opt_val.unpack() {
        ValueRef::Str(id) => {
            assert!(id.is_old());
            id.index()
        }
        _ => panic!("expected Str"),
    };
    assert!(
        new_idx < h.old().strings.len(),
        "optional default index {new_idx} OOB (len {}); pre-major was {pre_idx}",
        h.old().strings.len()
    );
    assert_eq!(h.old().strings[new_idx].as_str(), "default-str");
}

/// Nursery closure whose arm body contains an OLD string literal is rewritten.
#[test]
fn nursery_closure_old_body_literal_rewritten() {
    use crate::core::value::{Closure, ClosureArm};
    let mut h = Heap::new();
    let keeper = inflate_old_with_keeper_string(&mut h, 20, "body-literal");
    let pre_idx = match keeper.unpack() {
        ValueRef::Str(id) => id.index(),
        _ => unreachable!(),
    };

    let cl = Closure {
        name: None,
        arms: vec![ClosureArm {
            params: vec![],
            optionals: vec![],
            rest: None,
            body: vec![keeper], // OLD string in body
            passthrough: None,
        }]
        .into(),
        doc: None,
        env: None,
        module: None,
        identity: crate::core::value::fresh_closure_identity(),
    };
    let cl_id = h.alloc_closure(cl);
    let mut roots = [Value::func(cl_id)];
    h.minor_collect(false, &mut roots, &mut []);
    h.major_collect(&mut roots, &mut []);

    let new_cl_id = match roots[0].unpack() {
        ValueRef::Fn(id) => id,
        _ => unreachable!(),
    };
    let body_val = h.local.closures[new_cl_id.index()].arms[0].body[0];
    let new_idx = match body_val.unpack() {
        ValueRef::Str(id) => {
            assert!(id.is_old());
            id.index()
        }
        _ => panic!("expected Str"),
    };
    assert!(
        new_idx < h.old().strings.len(),
        "body literal index {new_idx} OOB (len {}); pre-major was {pre_idx}",
        h.old().strings.len()
    );
    assert_eq!(h.old().strings[new_idx].as_str(), "body-literal");
}

/// Nursery map (CHAMP root) with an OLD string as a data value is rewritten.
/// The map is accessed via map_get after the major; it must not panic/return None.
#[test]
fn nursery_map_old_data_value_rewritten() {
    let mut h = Heap::new();
    let keeper = inflate_old_with_keeper_string(&mut h, 20, "map-value");
    let pre_idx = match keeper.unpack() {
        ValueRef::Str(id) => id.index(),
        _ => unreachable!(),
    };
    let key = crate::core::value::sym("k");

    let empty_id = match h.alloc_empty_map().unpack() {
        ValueRef::Map(id) => id,
        _ => unreachable!(),
    };
    let mut roots = [h.map_assoc(empty_id, key, keeper)];
    h.minor_collect(false, &mut roots, &mut []); // flip — map stays nursery
    h.major_collect(&mut roots, &mut []); // compact — keeper only via map

    let map_id = match roots[0].unpack() {
        ValueRef::Map(id) => id,
        _ => unreachable!(),
    };
    let result = h.map_get(map_id, key).expect("key must survive flip+major");
    let new_idx = match result.unpack() {
        ValueRef::Str(id) => {
            assert!(id.is_old());
            id.index()
        }
        _ => panic!("expected Str, got {result:?}"),
    };
    assert!(
        new_idx < h.old().strings.len(),
        "map value index {new_idx} OOB (len {}); pre-major was {pre_idx}",
        h.old().strings.len()
    );
    assert_eq!(h.old().strings[new_idx].as_str(), "map-value");
}

/// Nursery pair with an OLD string car is rewritten after flip+major.
#[test]
fn nursery_pair_old_car_rewritten() {
    let mut h = Heap::new();
    let keeper = inflate_old_with_keeper_string(&mut h, 20, "pair-car");
    let pre_idx = match keeper.unpack() {
        ValueRef::Str(id) => id.index(),
        _ => unreachable!(),
    };

    // Nursery pair: car = OLD string, cdr = nil.
    let mut roots = [h.alloc_pair(keeper, Value::nil())];
    h.minor_collect(false, &mut roots, &mut []);
    h.major_collect(&mut roots, &mut []);

    let (car, _) = match roots[0].unpack() {
        ValueRef::Pair(id) => {
            assert!(!id.is_old(), "pair must still be nursery after flip");
            h.local.pairs[id.index()]
        }
        _ => unreachable!(),
    };
    let new_idx = match car.unpack() {
        ValueRef::Str(id) => {
            assert!(id.is_old());
            id.index()
        }
        _ => panic!("expected Str"),
    };
    assert!(
        new_idx < h.old().strings.len(),
        "pair car index {new_idx} OOB (len {}); pre-major was {pre_idx}",
        h.old().strings.len()
    );
    assert_eq!(h.old().strings[new_idx].as_str(), "pair-car");
}

/// Nursery vector with an OLD string element is rewritten after flip+major.
#[test]
fn nursery_vector_old_elem_rewritten() {
    let mut h = Heap::new();
    let keeper = inflate_old_with_keeper_string(&mut h, 20, "vec-elem");
    let pre_idx = match keeper.unpack() {
        ValueRef::Str(id) => id.index(),
        _ => unreachable!(),
    };

    let mut roots = [h.alloc_vector(vec![keeper])];
    h.minor_collect(false, &mut roots, &mut []);
    h.major_collect(&mut roots, &mut []);

    let vec_id = match roots[0].unpack() {
        ValueRef::Vector(id) => id,
        _ => unreachable!(),
    };
    let elem = h.local.vectors[vec_id.index()][0];
    let new_idx = match elem.unpack() {
        ValueRef::Str(id) => {
            assert!(id.is_old());
            id.index()
        }
        _ => panic!("expected Str"),
    };
    assert!(
        new_idx < h.old().strings.len(),
        "vector elem index {new_idx} OOB (len {}); pre-major was {pre_idx}",
        h.old().strings.len()
    );
    assert_eq!(h.old().strings[new_idx].as_str(), "vec-elem");
}

/// Nursery env with an OLD parent env: the parent pointer is rewritten.
/// Ensures the env lookup chain stays intact after flip+major.
#[test]
fn nursery_env_old_parent_rewritten() {
    let mut h = Heap::new();
    let sym = crate::core::value::intern("v");
    // Inflate: 20 decoy envs + keeper parent env (all tenured).
    let mut decoy_envs = Vec::from_fn(20, |_| h.new_env(None));
    let parent_env = h.new_env(None);
    h.env_define(parent_env, sym, Value::int(42));
    decoy_envs.push(parent_env);
    h.minor_collect(true, &mut [], &mut decoy_envs);
    let parent_old = decoy_envs.pop().unwrap();
    assert!(parent_old.is_old());
    let pre_idx = parent_old.index();
    drop(decoy_envs);

    // Nursery child env whose parent is the OLD env.
    let child_env = h.new_env(Some(parent_old));
    let mut er = vec![child_env];
    h.minor_collect(false, &mut [], &mut er);
    let _child_env = er[0];
    h.major_collect(&mut [], &mut er);
    let child_env = er[0];

    // Parent pointer in the nursery child must be rewritten.
    let parent_ptr = h.local.envs[child_env.index()]
        .parent
        .expect("child must have parent");
    assert!(
        parent_ptr.is_old(),
        "parent must still be old-gen after major"
    );
    assert!(
        parent_ptr.index() < h.old().envs.len(),
        "parent index {} OOB (len {}); pre-major was {pre_idx}",
        parent_ptr.index(),
        h.old().envs.len()
    );
    // The env lookup must traverse the parent chain correctly.
    let val = h
        .env_get(child_env, sym)
        .expect("v must be findable via parent");
    assert!(matches!(val.unpack(), ValueRef::Int(42)));
}

/// Nursery map whose root node has an OLD child sub-node (structural sharing
/// from `assoc` on an OLD map): the child pointer is rewritten.
#[test]
fn nursery_map_old_child_node_rewritten() {
    let mut h = Heap::new();
    // Build a map large enough for the trie to have child sub-nodes.
    // We use integer keys 0..32 to force at least one level of branching.
    let sym_key = crate::core::value::sym("new-key");
    let keeper_str = inflate_old_with_keeper_string(&mut h, 8, "map-child-keeper");
    // Build a base map with 32 entries, tenure it.
    let empty_id = match h.alloc_empty_map().unpack() {
        ValueRef::Map(id) => id,
        _ => unreachable!(),
    };
    let mut base = Value::map(empty_id);
    for i in 0..32i64 {
        let kid = match base.unpack() {
            ValueRef::Map(id) => id,
            _ => unreachable!(),
        };
        base = h.map_assoc(kid, Value::int(i), Value::int(i * 10));
    }
    let mut roots = [base];
    h.minor_collect(true, &mut roots, &mut []); // tenure base map into old
    let old_base_map = roots[0];
    // `assoc` on the OLD map creates a nursery root that shares OLD child nodes.
    let old_map_id = match old_base_map.unpack() {
        ValueRef::Map(id) => {
            assert!(id.is_old());
            id
        }
        _ => unreachable!(),
    };
    let nursery_map = h.map_assoc(old_map_id, sym_key, keeper_str);
    // Flip minor: drop old_base_map root; only nursery map keeps base alive.
    let mut roots = [nursery_map];
    h.minor_collect(false, &mut roots, &mut []);
    h.major_collect(&mut roots, &mut []);

    // map_get on the surviving nursery map must traverse correctly after the major.
    let final_id = match roots[0].unpack() {
        ValueRef::Map(id) => id,
        _ => unreachable!(),
    };
    // The newly-added key must resolve.
    let val = h
        .map_get(final_id, sym_key)
        .expect("new-key must survive flip+major");
    match val.unpack() {
        ValueRef::Str(id) => {
            assert!(id.is_old());
            assert!(id.index() < h.old().strings.len(), "string OOB after major");
            assert_eq!(h.old().strings[id.index()].as_str(), "map-child-keeper");
        }
        _ => panic!("expected Str"),
    }
    // An original key from the shared sub-tree must also resolve.
    let orig = h
        .map_get(final_id, Value::int(0))
        .expect("key 0 must survive");
    assert!(matches!(orig.unpack(), ValueRef::Int(0)));
}

/// Multiple nursery objects across all five affected types hold OLD references
/// to the same keeper string; all must be rewritten in one major pass.
#[test]
fn all_nursery_types_old_refs_rewritten_together() {
    use crate::core::value::{Closure, ClosureArm};
    let mut h = Heap::new();
    let sym = crate::core::value::intern("k");
    let keeper = inflate_old_with_keeper_string(&mut h, 20, "shared-keeper");
    let pre_idx = match keeper.unpack() {
        ValueRef::Str(id) => id.index(),
        _ => unreachable!(),
    };

    // Five nursery objects each referencing the same OLD keeper.
    let nursery_env = h.new_env(None);
    h.env_define(nursery_env, sym, keeper);

    let cl = Closure {
        name: None,
        arms: vec![ClosureArm {
            params: vec![],
            optionals: vec![(sym, keeper)],
            rest: None,
            body: vec![keeper],
            passthrough: None,
        }]
        .into(),
        doc: None,
        env: None,
        module: None,
        identity: crate::core::value::fresh_closure_identity(),
    };
    let cl_val = Value::func(h.alloc_closure(cl));

    let empty_id = match h.alloc_empty_map().unpack() {
        ValueRef::Map(id) => id,
        _ => unreachable!(),
    };
    let map_val = h.map_assoc(empty_id, crate::core::value::sym("m"), keeper);
    let pair_val = h.alloc_pair(keeper, Value::nil());
    let vec_val = h.alloc_vector(vec![keeper]);

    let mut vr = [cl_val, map_val, pair_val, vec_val];
    let mut er = vec![nursery_env];
    h.minor_collect(false, &mut vr, &mut er);
    h.major_collect(&mut vr, &mut er);

    let check_str = |val: Value, label: &str| {
        let new_idx = match val.unpack() {
            ValueRef::Str(id) => {
                assert!(id.is_old(), "{label}: must be old-gen");
                id.index()
            }
            _ => panic!("{label}: expected Str"),
        };
        assert!(
            new_idx < h.old().strings.len(),
            "{label}: index {new_idx} OOB (len {}); pre-major was {pre_idx}",
            h.old().strings.len()
        );
        assert_eq!(
            h.old().strings[new_idx].as_str(),
            "shared-keeper",
            "{label}: wrong content"
        );
    };

    // Env var.
    let nursery_env = er[0];
    let env_val = h.local.envs[nursery_env.index()]
        .vars
        .iter()
        .find(|(s, _)| *s == sym)
        .map(|(_, v)| *v)
        .unwrap();
    check_str(env_val, "env var");

    // Closure optional default + body literal.
    let new_cl_id = match vr[0].unpack() {
        ValueRef::Fn(id) => id,
        _ => unreachable!(),
    };
    let cl = &h.local.closures[new_cl_id.index()];
    check_str(cl.arms[0].optionals[0].1, "closure optional");
    check_str(cl.arms[0].body[0], "closure body");

    // Map value.
    let map_id = match vr[1].unpack() {
        ValueRef::Map(id) => id,
        _ => unreachable!(),
    };
    let map_result = h.map_get(map_id, crate::core::value::sym("m")).unwrap();
    check_str(map_result, "map value");

    // Pair car.
    let pair_id = match vr[2].unpack() {
        ValueRef::Pair(id) => id,
        _ => unreachable!(),
    };
    let (car, _) = h.local.pairs[pair_id.index()];
    check_str(car, "pair car");

    // Vector element.
    let vec_id = match vr[3].unpack() {
        ValueRef::Vector(id) => id,
        _ => unreachable!(),
    };
    check_str(h.local.vectors[vec_id.index()][0], "vector elem");
}

/// Regression: a **major collection must not discard the source positions of live
/// NURSERY forms.** The major re-keys `form_pos` through the old-gen forwarding
/// table; it must keep every young entry, since a major does not move the nursery.
///
/// The interleave below is the reachable one: tenure an old form, read a young one
/// (flip minor keeps it young), then major.
#[test]
fn major_keeps_nursery_form_positions() {
    use crate::error::Pos;
    let mut h = Heap::new();
    let old_pos = Pos { line: 9, col: 5 };
    let young_pos = Pos { line: 7, col: 3 };

    // A form that tenures into the old generation.
    let old_form = h.alloc_pair(Value::int(1), Value::nil());
    h.set_form_pos(old_form, old_pos);
    let mut roots = vec![old_form];
    h.minor_collect(true, &mut roots, &mut []); // tenure
    let old_form = roots[0];
    assert!(
        matches!(old_form.unpack(), ValueRef::Pair(id) if id.is_old()),
        "the first form should have tenured",
    );

    // A form read *after* the tenure: still in the nursery.
    let young_form = h.alloc_pair(Value::int(2), Value::nil());
    h.set_form_pos(young_form, young_pos);

    // A FLIP minor (survivors stay young) — the nursery entry is re-keyed here.
    let mut roots = vec![old_form, young_form];
    h.minor_collect(false, &mut roots, &mut []);
    let (old_form, young_form) = (roots[0], roots[1]);
    assert_eq!(h.form_pos_only(young_form), Some(young_pos));

    // The major: compacts the old generation, leaves the nursery alone.
    let mut roots = vec![old_form, young_form];
    h.major_collect(&mut roots, &mut []);
    let (old_form, young_form) = (roots[0], roots[1]);

    assert_eq!(
        h.form_pos_only(young_form),
        Some(young_pos),
        "a major dropped the position of a live NURSERY form (it never moved it)",
    );
    assert_eq!(
        h.form_pos_only(old_form),
        Some(old_pos),
        "a major lost the re-keyed position of a surviving OLD form",
    );
}

/// The contract's verdict memo (ADR-480) keys on a handle's raw bits, so everything that
/// lets those bits name a different object must forget what they named. `reset_local_to`
/// truncates the nursery and mints the SAME slot at the SAME epoch for the next allocation:
/// a verdict kept across it would answer for an object it never saw.
#[test]
fn reset_local_to_forgets_verdicts_about_reclaimed_slots() {
    use crate::core::heap::KnownVerdict;
    let mut h = Heap::new();
    let type_expression = Value::Sym(crate::core::value::intern("verdict-test-type"));
    let checkpoint = h.checkpoint();
    let first = h.alloc_vector(vec![Value::int(1)]);
    h.record_type_verdict(type_expression, first, true);
    assert!(matches!(
        h.type_verdict(type_expression, first),
        KnownVerdict::Known(true)
    ));
    h.reset_local_to(checkpoint);
    let reused = h.alloc_vector(vec![Value::nil()]);
    assert_eq!(
        format!("{first:?}"),
        format!("{reused:?}"),
        "the reclaimed slot should be minted again — the case this guards",
    );
    assert!(
        matches!(
            h.type_verdict(type_expression, reused),
            KnownVerdict::Unknown
        ),
        "a verdict about the reclaimed vector answered for the one now in its slot",
    );
}

/// A minor collection moves every nursery survivor, so a verdict about a nursery object is
/// dropped; one about an OLD object stays, because a minor never moves it — which is what
/// lets an editor's tenured model answer from the memo across collections. A major
/// compacts the old generation, and its verdicts go too.
#[test]
fn collections_forget_exactly_the_verdicts_whose_objects_move() {
    use crate::core::heap::KnownVerdict;
    let mut h = Heap::new();
    let type_expression = Value::Sym(crate::core::value::intern("verdict-test-type"));
    let tenured = h.alloc_vector(vec![Value::int(1)]);
    let mut roots = vec![tenured];
    h.minor_collect(true, &mut roots, &mut []);
    let tenured = roots[0];
    let young = h.alloc_vector(vec![Value::int(2)]);
    h.record_type_verdict(type_expression, tenured, true);
    h.record_type_verdict(type_expression, young, false);
    assert_eq!(h.type_verdict_counts().entries, 2);

    let mut roots = vec![tenured, young];
    h.minor_collect(false, &mut roots, &mut []);
    let (tenured, young) = (roots[0], roots[1]);
    assert_eq!(
        h.type_verdict_counts().entries,
        1,
        "the nursery verdict survived a minor"
    );
    assert!(matches!(
        h.type_verdict(type_expression, tenured),
        KnownVerdict::Known(true)
    ));
    assert!(matches!(
        h.type_verdict(type_expression, young),
        KnownVerdict::Unknown
    ));

    let mut roots = vec![tenured, young];
    h.major_collect(&mut roots, &mut []);
    assert_eq!(
        h.type_verdict_counts().entries,
        0,
        "an old-generation verdict survived a major"
    );
}

/// A declared type changing (a new or redefined `deftype`) or any global rebinding (a
/// `defrecord` growing `*record-ids*`, a sealed ability gaining members) can change the
/// verdict for the very same pair, so either starts the memo over.
#[test]
fn a_declaration_or_a_rebinding_starts_the_verdicts_over() {
    use crate::core::heap::KnownVerdict;
    use std::sync::atomic::Ordering;
    let mut h = Heap::new();
    let type_expression = Value::Sym(crate::core::value::intern("verdict-test-type"));
    let value = h.alloc_vector(vec![Value::int(1)]);
    h.record_type_verdict(type_expression, value, true);
    h.runtime
        .declared_sigs_version
        .fetch_add(1, Ordering::AcqRel);
    assert!(matches!(
        h.type_verdict(type_expression, value),
        KnownVerdict::Unknown
    ));
    h.record_type_verdict(type_expression, value, true);
    h.runtime.version.fetch_add(1, Ordering::Relaxed);
    assert!(matches!(
        h.type_verdict(type_expression, value),
        KnownVerdict::Unknown
    ));
}

/// An old generation of a few objects that own large dead buffers is compacted on its
/// bytes (KI-307). Each round tenures one 12 MB vector and drops the previous one, so the
/// old generation's object count stays far below the major floor while its dead buffers
/// pile up; only the byte half of the major trigger reclaims them. Driven through
/// `collect`, the entry point the eval safepoint calls.
#[test]
fn an_old_generation_of_dead_large_vectors_is_compacted_on_its_bytes() {
    const ELEMENTS: usize = 512 * 1024; // 12 MB of `Value`s
    const ROUNDS: usize = 12;
    let mut h = Heap::new();
    h.gc_enabled = true;
    h.major_threshold = major_floor();
    let mut roots = [Value::nil()];
    for round in 0..ROUNDS {
        roots[0] = h.alloc_vector(vec![Value::int(round as i64); ELEMENTS]);
        // Garbage enough that the collection tenures its survivor.
        for filler in 0..min_tenure() {
            h.alloc_pair(Value::int(filler as i64), Value::nil());
        }
        h.collect(&mut roots, &mut []);
    }
    assert!(
        h.old_live_count() < major_floor(),
        "the object count alone would have compacted"
    );
    let retained = buffer_bytes_since(h.old(), BufferMark::default());
    let one_vector = ELEMENTS * std::mem::size_of::<Value>();
    assert!(
        retained < ROUNDS / 2 * one_vector,
        "the old generation keeps {} MB of vectors after {ROUNDS} rounds of one live vector",
        retained >> 20
    );
    match roots[0].unpack() {
        ValueRef::Vector(id) => {
            let survivor = h.vector(id);
            assert_eq!(survivor.len(), ELEMENTS);
            assert!(
                matches!(survivor[0].unpack(), ValueRef::Int(round) if round == ROUNDS as i64 - 1)
            );
        }
        _ => unreachable!(),
    }
}

/// The handle a `Value::Vector` holds, for the append tests.
fn vector_id(v: Value) -> VecId {
    match v.unpack() {
        ValueRef::Vector(id) => id,
        _ => panic!("not a vector"),
    }
}

/// A vector's elements as integers, for the append tests.
fn ints(h: &Heap, v: Value) -> Vec<i64> {
    h.vector(vector_id(v))
        .iter()
        .map(|x| match x.unpack() {
            ValueRef::Int(i) => i,
            _ => panic!("not an int"),
        })
        .collect()
}

/// ADR-511: `vector_append` writes into the spare capacity of an append buffer only for the
/// newest vector over it, and never changes a vector that already exists. A first append
/// copies exactly; appending to that copies with room; the next appends go in place; and an
/// append onto a version that something was already appended to copies, so two `conj`s onto
/// one vector cannot both claim the same slot.
#[test]
fn an_append_writes_in_place_only_past_the_newest_vector() {
    let mut h = Heap::new();
    let base = h.alloc_vector((0..10).map(Value::int).collect());
    let a = h.vector_append(vector_id(base), &[Value::int(10)]);
    let b = h.vector_append(vector_id(a), &[Value::int(11)]);
    let c = h.vector_append(vector_id(b), &[Value::int(12)]);
    assert_eq!(
        h.vector(vector_id(b)).as_ptr(),
        h.vector(vector_id(c)).as_ptr(),
        "the append onto the newest vector went in place"
    );
    let d = h.vector_append(vector_id(b), &[Value::int(99)]);
    assert_ne!(
        h.vector(vector_id(b)).as_ptr(),
        h.vector(vector_id(d)).as_ptr(),
        "a second append onto the same vector copied"
    );
    assert_eq!(ints(&h, base), (0..10).collect::<Vec<_>>());
    assert_eq!(ints(&h, a), (0..11).collect::<Vec<_>>());
    assert_eq!(ints(&h, b), (0..12).collect::<Vec<_>>());
    assert_eq!(ints(&h, c), (0..13).collect::<Vec<_>>());
    let mut expected_d: Vec<i64> = (0..12).collect();
    expected_d.push(99);
    assert_eq!(ints(&h, d), expected_d);
}

/// ADR-511: a collection relocates every vector over a shared append buffer with its own
/// elements intact — the young ones that share a buffer, and a young vector appended into
/// the buffer of a tenured one (an old buffer then holds a young element past the old
/// vector's length, which only the young vector reads). A minor, then a major.
#[test]
fn a_collection_keeps_every_vector_over_a_shared_buffer() {
    let mut h = Heap::new();
    h.gc_enabled = true;
    let mut roots = [Value::nil(); 4];
    let start = h.alloc_vector((0..10).map(Value::int).collect());
    let first = h.vector_append(vector_id(start), &[Value::int(10)]);
    roots[0] = h.vector_append(vector_id(first), &[Value::int(11)]); // room to spare
    // Tenure it: enough garbage that the collection moves survivors to the old generation.
    for filler in 0..min_tenure() {
        h.alloc_pair(Value::int(filler as i64), Value::nil());
    }
    h.collect(&mut roots, &mut []);
    assert!(vector_id(roots[0]).is_old(), "the vector was tenured");
    // Young appends into the tenured vector's buffer, and a branch that copies.
    roots[1] = h.vector_append(vector_id(roots[0]), &[Value::int(12)]);
    roots[2] = h.vector_append(vector_id(roots[1]), &[Value::int(13)]);
    roots[3] = h.vector_append(vector_id(roots[1]), &[Value::int(99)]);
    let check = |h: &Heap, roots: &[Value; 4]| {
        assert_eq!(ints(h, roots[0]), (0..12).collect::<Vec<_>>());
        assert_eq!(ints(h, roots[1]), (0..13).collect::<Vec<_>>());
        assert_eq!(ints(h, roots[2]), (0..14).collect::<Vec<_>>());
        let mut branch: Vec<i64> = (0..13).collect();
        branch.push(99);
        assert_eq!(ints(h, roots[3]), branch);
    };
    check(&h, &roots);
    h.minor_collect(false, &mut roots, &mut []);
    check(&h, &roots);
    h.major_collect(&mut roots, &mut []);
    check(&h, &roots);
    // And an append after the collections still sees the right elements.
    let after = h.vector_append(vector_id(roots[2]), &[Value::int(14)]);
    assert_eq!(ints(&h, after), (0..15).collect::<Vec<_>>());
}

/// ADR-511: writable access to a vector over a buffer another vector holds gives it a buffer
/// of its own first, so a builder or the collector writing through it never reaches the
/// other.
#[test]
fn a_write_never_reaches_another_vector_over_the_same_buffer() {
    let mut h = Heap::new();
    let start = h.alloc_vector((0..10).map(Value::int).collect());
    let first = h.vector_append(vector_id(start), &[Value::int(10)]);
    let shorter = h.vector_append(vector_id(first), &[Value::int(11)]);
    let longer = h.vector_append(vector_id(shorter), &[Value::int(12)]);
    let index = vector_id(shorter).index();
    h.local.vectors[index][0] = Value::int(-1);
    assert_eq!(ints(&h, shorter)[0], -1);
    assert_eq!(
        ints(&h, longer),
        (0..13).collect::<Vec<_>>(),
        "a write through one vector reached another over the same buffer"
    );
}

/// ADR-511: the vector a `conj` loop is building keeps its room across a collection. Its
/// older versions are garbage still sitting in the nursery, so when the collector reaches
/// it the buffer is still shared and it is copied out — into a buffer as roomy as the one
/// it leaves, being the newest over it, so the loop's next `conj` goes in place.
#[test]
fn the_vector_a_loop_is_building_keeps_its_room_across_a_collection() {
    let mut h = Heap::new();
    h.gc_enabled = true;
    let mut newest = h.alloc_vector((0..10).map(Value::int).collect());
    for i in 10..40 {
        newest = h.vector_append(vector_id(newest), &[Value::int(i)]);
    }
    let mut roots = [newest];
    h.collect(&mut roots, &mut []);
    let kept = roots[0];
    let next = h.vector_append(vector_id(kept), &[Value::int(40)]);
    assert_eq!(
        h.vector(vector_id(kept)).as_ptr(),
        h.vector(vector_id(next)).as_ptr(),
        "the loop's next conj after the collection copied"
    );
    assert_eq!(ints(&h, kept), (0..40).collect::<Vec<_>>());
    assert_eq!(ints(&h, next), (0..41).collect::<Vec<_>>());
}

/// ADR-511: when the collector finds itself the only holder of an append buffer, it cuts
/// the buffer back to the vector's length — the slots past it were claimed by longer
/// vectors that have since died — so the vector's next `conj` goes in place. Here a tenured
/// vector's buffer gains a young vector that dies in a minor; the major after it finds the
/// buffer held by the tenured vector alone.
#[test]
fn a_buffer_the_collector_holds_alone_is_cut_back_to_its_vector() {
    let mut h = Heap::new();
    h.gc_enabled = true;
    let start = h.alloc_vector((0..10).map(Value::int).collect());
    let first = h.vector_append(vector_id(start), &[Value::int(10)]);
    let mut roots = [h.vector_append(vector_id(first), &[Value::int(11)])];
    for filler in 0..min_tenure() {
        h.alloc_pair(Value::int(filler as i64), Value::nil());
    }
    h.collect(&mut roots, &mut []);
    assert!(vector_id(roots[0]).is_old(), "the vector was tenured");
    // A longer young vector over the tenured one's buffer, which dies in the next minor.
    h.vector_append(vector_id(roots[0]), &[Value::int(12)]);
    h.minor_collect(false, &mut roots, &mut []);
    h.major_collect(&mut roots, &mut []);
    let kept = roots[0];
    let next = h.vector_append(vector_id(kept), &[Value::int(42)]);
    assert_eq!(
        h.vector(vector_id(kept)).as_ptr(),
        h.vector(vector_id(next)).as_ptr(),
        "the append after the major copied: the dead vector's slot was still claimed"
    );
    let mut expected: Vec<i64> = (0..12).collect();
    assert_eq!(ints(&h, kept), expected);
    expected.push(42);
    assert_eq!(ints(&h, next), expected);
}

/// ADR-670: a `defmemo` store is a root of its process's heap. Its entries are held
/// uncopied, so the collector must relocate them — through a minor collection that keeps
/// them young, one that tenures them, and a major one that compacts the old generation —
/// and a `reset_local_to` that may reclaim them must forget the store.
#[test]
fn a_memo_store_is_relocated_by_every_collection_and_forgotten_by_a_reset() {
    let mut h = Heap::new();
    h.gc_enabled = true;
    let slot = crate::core::value::intern("memo-test/rows-of");
    let version = crate::core::value::intern("memo__1");
    let other_version = crate::core::value::intern("memo__2");

    // Garbage around the store, so a collection that skipped it would hand its slots on.
    let _dead = h.alloc_vector(vec![Value::int(-1); 8]);
    let key = h.alloc_vector(vec![Value::int(1), Value::int(2), Value::int(3)]);
    let value = Value::int(42);
    let entry = h.alloc_vector2(key, value);
    let entries = h.alloc_pair(entry, Value::nil());
    h.set_memo_store(slot, version, entries);

    let check = |h: &Heap| {
        let entries = h
            .memo_store(slot, Some(version))
            .expect("the store survives");
        let (entry, rest) = match entries.unpack() {
            ValueRef::Pair(id) => h.pair(id),
            _ => panic!("the entries are no longer a list"),
        };
        assert!(matches!(rest.unpack(), ValueRef::Nil));
        let fields = h.vector(vector_id(entry)).to_vec();
        assert_eq!(ints(h, fields[0]), vec![1, 2, 3]);
        assert!(matches!(fields[1].unpack(), ValueRef::Int(42)));
    };

    h.minor_collect(false, &mut [], &mut []);
    check(&h);
    h.minor_collect(true, &mut [], &mut []);
    check(&h);
    h.major_collect(&mut [], &mut []);
    check(&h);

    // Another version of the definition reads nothing; a nil version reads what is kept.
    assert!(h.memo_store(slot, Some(other_version)).is_none());
    assert!(h.memo_store(slot, None).is_some());

    // A nursery store from a form a tooling eval then rolls back is forgotten.
    let checkpoint = h.checkpoint();
    let young = h.alloc_pair(Value::int(7), Value::nil());
    h.set_memo_store(slot, version, young);
    h.reset_local_to(checkpoint);
    assert!(h.memo_store(slot, None).is_none());
}
