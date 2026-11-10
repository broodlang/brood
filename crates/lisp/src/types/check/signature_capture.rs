//! The effective-signature capture behind the LSP's inlay hints and `--suggest-sigs`:
//! [`file_signatures`] runs a full file check with a capture armed and returns each
//! function's declared or inferred [`FnSignature`], reshaped to its definition's arity so a
//! suggestion is writable.

use super::*;

/// What a file's functions are, as the checker sees them — the data behind the LSP's
/// **effective-type inlay hints** (`docs/lsp.md`).
#[derive(Clone, Debug)]
pub struct FnSignature {
    /// The name as written at the def site (unqualified).
    pub name: String,
    /// Its signature: what the author *declared*, else what the checker inferred.
    pub sig: Sig,
    /// True when a `(sig …)` states this — in which case the signature is already on
    /// screen, and a hint repeating it is noise.
    pub declared: bool,
}

/// Every function a file defines, with its **effective** signature.
///
/// This is the question a buffer wants answered and `hover` could not: hover reads the
/// *loaded* image, and a file being edited is not loaded. The checker has inferred
/// same-file functions from their FORMS since ADR-188/190 (and per-clause since
/// ADR-261) precisely because it cannot load them either — so the answer already
/// exists, one pass away, and this exposes it.
///
/// Runs the full [`check_file`] analysis with a capture armed and discards the
/// diagnostics, exactly as [`arg_ty_at`] does: one code path, so a hint can never
/// describe a different inference than the warnings do.
pub fn file_signatures(heap: &mut Heap, forms: &[Value]) -> Vec<FnSignature> {
    arm_signature_capture();
    let _ = check_file(heap, forms);
    take_signature_capture()
}

thread_local! {
    /// Armed by [`file_signatures`]; filled once the passes have built the `Ctx`.
    static SIGNATURE_CAPTURE: std::cell::RefCell<Option<Vec<FnSignature>>> =
        const { std::cell::RefCell::new(None) };
}

fn arm_signature_capture() {
    SIGNATURE_CAPTURE.with(|c| *c.borrow_mut() = Some(Vec::new()));
}

fn take_signature_capture() -> Vec<FnSignature> {
    SIGNATURE_CAPTURE
        .with(|c| c.borrow_mut().take())
        .unwrap_or_default()
}

/// Record each `defn`'s effective signature, if a capture is armed. Called once the
/// inference passes have run, so a declared sig, an inferred single arm and an
/// inferred multi-arm set are all visible.
pub(super) fn capture_file_signatures(heap: &Heap, expanded: &[Value], ctx: &Ctx) {
    SIGNATURE_CAPTURE.with(|c| {
        let mut slot = c.borrow_mut();
        let Some(out) = slot.as_mut() else {
            return; // not armed — the ordinary checking path pays nothing
        };
        for form in top_level_defs(heap, expanded) {
            let Some((name, rhs)) = def_name_and_value(heap, form) else {
                continue;
            };
            // Only functions: a `(def x 5)` has a value type, not a signature. And
            // never a macro's generated temporary — nobody can write a `(sig …)` for a
            // name they cannot refer to.
            if !walk::is_fn_value_form(heap, rhs)
                || ctx.is_file_macro(name)
                || value::is_gensym(name)
            {
                continue;
            }
            let (sig, declared) = match ctx.declared_sig(name) {
                Some(sig) => (sig, true),
                None => match inferred_signature(ctx, name) {
                    // An inferred signature is a *fact about types*; it carries no
                    // obligation to describe the definition's shape, and for a function
                    // whose params the checker could not type at all it is bare
                    // `(-> ret)`. Written into a file that way it declares a NULLARY
                    // function, which ADR-259's the declaration check then rejects as contradicting
                    // its `defn` — so a suggestion must never be offered in that state.
                    Some(sig) => (reconcile_arity(heap, rhs, sig), false),
                    None => continue,
                },
            };
            out.push(FnSignature {
                name: value::symbol_name(name),
                sig,
                declared,
            });
        }
    });
}

/// The most informative inferred signature for `name`.
///
/// A **multi-arm** function has one signature per arm (ADR-261) and, separately, a
/// params-less sig carrying the union of the arms' returns — because a guarded
/// multi-clause `defn` lowers to one variadic `fn`, so the per-position inference
/// declines while the return inference does not. Neither half alone is worth showing:
/// the first is `(string) -> any`, the second `() -> 1 | 2`. Take the first arm's
/// parameters and that union return, which is a sound over-approximation of what the
/// arm returns and the shape a reader is actually looking at.
fn inferred_signature(ctx: &Ctx, name: Symbol) -> Option<Sig> {
    let single = ctx.inferred_fn_sig(name);
    let Some(arms) = ctx.inferred_overload(name).filter(|a| !a.is_empty()) else {
        return single;
    };
    // Parameters: the position-wise union over every arm of the FIRST arm's arity — the
    // shape a reader is looking at, and a sound over-approximation of what those arms
    // admit. Return: the union of the arms' returns when the clause inference typed them,
    // else the params-less union the expanded tree gave.
    let arity = arms[0].params.len();
    let same_arity: Vec<&Sig> = arms.iter().filter(|a| a.params.len() == arity).collect();
    let params: Vec<Ty> = (0..arity)
        .map(|i| {
            same_arity
                .iter()
                .fold(Ty::NEVER, |acc, a| acc.union(a.params[i].clone()))
        })
        .collect();
    let arm_ret = arms
        .iter()
        .fold(Ty::NEVER, |acc, a| acc.union(a.ret.clone()));
    let ret = match &single {
        _ if !arm_ret.is_any() => arm_ret,
        Some(s) => s.ret.clone(),
        None => arm_ret,
    };
    Some(Sig::new(params, ret))
}

/// A signature reshaped to the definition's actual parameter list, filling anything the
/// inference did not type with `any`.
///
/// This is what makes a suggestion *writable*: the LSP's declare-sig action and
/// `nest check --suggest-sigs` both hand a `(sig …)` to a human to paste, and one whose
/// arity contradicts its `defn` is not a weaker suggestion — it is a broken one that the
/// declaration checker rejects. `string/last-index-of (s needle &optional before)`
/// inferred `(-> int)`; it now reads `(any any &optional any -> int)`, which says less
/// about the parameters and the truth about the shape.
fn reconcile_arity(heap: &Heap, value_form: Value, sig: Sig) -> Sig {
    let Some(arity) = walk::fn_form_arity(heap, value_form) else {
        return sig;
    };
    let required = arity.min;
    // **Fill in, never overrule.** A multi-clause `defn` lowers to a single variadic
    // `fn` over `match*` (ADR-226), so the *form's* arity says "any number of arguments"
    // while the clause inference knows the real one — reshaping to the form would throw
    // that away and offer `(...any) -> …`. So this only acts on the case it exists for:
    // an inference that produced no parameters at all for a definition that has some.
    if sig.params.len() >= required {
        return sig;
    }
    let mut params: Vec<Ty> = (0..required)
        .map(|i| sig.params.get(i).cloned().unwrap_or(Ty::ANY))
        .collect();
    let mut optional = sig.optional.clone();
    let mut rest = sig.rest.clone();
    match arity.max {
        // Unbounded: the tail is a rest binder.
        None => rest = rest.or(Some(Ty::ANY)),
        Some(max) => {
            let extra = max.saturating_sub(required);
            while optional.len() < extra {
                optional.push(Ty::ANY);
            }
        }
    }
    params.truncate(required);
    Sig {
        params,
        optional,
        rest,
        ret: sig.ret,
        guard: sig.guard,
    }
}
