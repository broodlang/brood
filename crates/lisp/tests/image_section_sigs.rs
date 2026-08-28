//! A NAMED image section must carry its own declared `sig`s.
//!
//! The regression this exists for. `encode_section` wrote sigs only when the section name
//! was EMPTY — the root/project image's shape, and for a long time the only shape there was.
//! The stdlib image (ADR-256) is written as one **named** section per module and never
//! writes an unnamed one, so it carried no signatures at all: a module restored from it came
//! back fully bound and completely **unsigned**.
//!
//! Nothing failed loudly. What happened instead is the failure mode this repo spent a whole
//! day on — a gate that stops gating. With the image installed, `nest check` lost every std
//! signature, so the checker's reversed-argument lint had nothing to compare against and
//! `types::check::tests::a_reversed_index_and_collection_call_is_flagged` passed silently.
//! Deterministic: 3 of 3 runs failed with the image, 0 of 3 without.

use brood::core::value;
use brood::Interp;

fn tmp_image(tag: &str) -> String {
    std::env::temp_dir()
        .join(format!("brood-image-sigs-{}-{tag}.bin", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn a_named_section_restores_the_sigs_of_its_own_symbols() {
    let path = tmp_image("named");
    let mut w = Interp::new();
    let written = w
        .eval_str(&format!(
            r#"
        (defn zz-image-sig-probe (a b) a)
        (sig zz-image-sig-probe (int string -> int))
        ;; A second SIGNED name that the section does not list. It must NOT ride along:
        ;; over-including would put the whole sig table into each of ~100 module sections,
        ;; which is silent bloat rather than a failure — the same mistake in the other
        ;; direction, and the count alone cannot see it (a fresh runtime's prelude sigs live
        ;; in the frozen bundle, not in this snapshot, so the totals stay small either way).
        (defn zz-image-sig-outsider (a) a)
        (sig zz-image-sig-outsider (int -> int))
        (%image-write "{path}" [["mymod" ['zz-image-sig-probe]]] "sig-fp")
        "#
        ))
        .expect("writing the probe image");
    // ITS OWN sigs, not every sig in the runtime. One binding + one sig = 2 entries; the
    // prelude has hundreds of signed names, so a section that took them all would be in the
    // hundreds here. Without this bound the fix could over-correct into writing the whole
    // sig table into every one of ~100 module sections, which is silent bloat rather than a
    // failure — the same shape of mistake in the opposite direction.
    let n: i64 = w.print(written).parse().unwrap_or(-1);
    assert_eq!(
        n, 2,
        "a named section wrote {n} entries for one signed symbol — it should carry its own \
         binding and its own sig, nothing else"
    );

    // A fresh runtime: the sig can only be present if the SECTION carried it.
    let mut r = Interp::new();
    let sym = value::intern("zz-image-sig-probe");
    assert!(
        r.heap.declared_sig_value(sym).is_none(),
        "the probe name must not be signed before the section is loaded — otherwise this \
         test cannot tell a restored sig from an ambient one"
    );
    r.eval_str(&format!(
        r#"
        (let (idx (%image-index "{path}" "sig-fp") sec (get idx "mymod"))
          (%image-load-section "{path}" (first sec) (second sec)))
        "#
    ))
    .expect("loading the probe section");

    assert!(
        r.heap.env_get(brood::core::value::EnvId::GLOBAL, sym).is_some(),
        "the binding itself did not restore — the section is wrong, not just its sigs"
    );
    assert!(
        r.heap.declared_sig_value(sym).is_some(),
        "a NAMED section dropped its symbol's declared sig. That is silent: the module is \
         bound and callable, and only the checker notices — by no longer catching anything."
    );
    assert!(
        r.heap
            .declared_sig_value(value::intern("zz-image-sig-outsider"))
            .is_none(),
        "a named section carried a sig for a symbol it does not list — over-inclusion would \
         copy the entire sig table into every module section"
    );
    let _ = std::fs::remove_file(&path);
}

/// The root/project section keeps taking EVERY declared sig, which is what it did before and
/// what a project image relies on — its one unnamed section carries the whole image.
#[test]
fn the_unnamed_root_section_still_takes_every_sig() {
    let path = tmp_image("root");
    let mut w = Interp::new();
    w.eval_str(&format!(
        r#"
        (defn zz-root-sig-probe (a) a)
        (sig zz-root-sig-probe (int -> int))
        ;; note: the section lists a DIFFERENT symbol, so the sig can only arrive via the
        ;; unnamed section's take-everything rule.
        (defn zz-root-other (a) a)
        (%image-write "{path}" [["" ['zz-root-other]]] "root-fp")
        "#
    ))
    .expect("writing the root image");

    let mut r = Interp::new();
    r.eval_str(&format!(
        r#"
        (let (idx (%image-index "{path}" "root-fp") sec (get idx ""))
          (%image-load-section "{path}" (first sec) (second sec)))
        "#
    ))
    .expect("loading the root section");

    assert!(
        r.heap
            .declared_sig_value(value::intern("zz-root-sig-probe"))
            .is_some(),
        "the unnamed root section stopped carrying every sig — that is the project image's \
         contract, and narrowing it here would break project images to fix the stdlib one"
    );
    let _ = std::fs::remove_file(&path);
}
