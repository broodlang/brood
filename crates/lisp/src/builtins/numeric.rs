use super::realize_seqview;
use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::core::value::Tag;
    use crate::types::Sig;
    use crate::types::Ty;
    // numeric primitives — `%add`..`%div` accept and return the wider NUMBER
    // (int + int may overflow into Float; the others always do on a Float arg).
    // `%lt` is comparison → bool; `%eq` accepts anything and returns bool.
    primitives.def(
        "%add",
        Arity::exact(2),
        Sig::new(vec![num, num], num),
        &[],
        "",
        prim_add,
    );
    primitives.def(
        "%sub",
        Arity::exact(2),
        Sig::new(vec![num, num], num),
        &[],
        "",
        prim_sub,
    );
    primitives.def(
        "%mul",
        Arity::exact(2),
        Sig::new(vec![num, num], num),
        &[],
        "",
        prim_mul,
    );
    primitives.def(
        "%div",
        Arity::exact(2),
        Sig::new(vec![num, num], num),
        &[],
        "",
        prim_div,
    );
    primitives.def(
        "%lt",
        Arity::exact(2),
        Sig::new(vec![num, num], bool_ty),
        &[],
        "",
        prim_lt,
    );
    primitives.def(
        "%le",
        Arity::exact(2),
        Sig::new(vec![num, num], bool_ty),
        &[],
        "",
        prim_le,
    );
    // `min`/`max` accept a number OR an `Ord` record — they route through the `compare-to`
    // multimethod on the record cold path (ADR-179), returning the same domain.
    let num_or_record = num.union(map_ty);
    primitives.def(
        "%max",
        Arity::at_least(1),
        Sig::variadic(num_or_record.clone(), num_or_record.clone()),
        &["x", "&", "more"],
        "The greatest of one or more numbers (int/float/decimal), compared numerically; the result keeps its own type.",
        prim_max);
    primitives.def(
        "%min",
        Arity::at_least(1),
        Sig::variadic(num_or_record.clone(), num_or_record),
        &["x", "&", "more"],
        "The least of one or more numbers (int/float/decimal), compared numerically; the result keeps its own type.",
        prim_min);
    primitives.def(
        kw::EQ_PRIM,
        Arity::exact(2),
        Sig::new(vec![any, any], bool_ty),
        &[],
        "",
        prim_eq,
    );
    // `mod` is Brood over `rem` (std/prelude.blsp); only `rem` is primitive.
    primitives.def(
        "%rem",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        &["a", "b"],
        "Integer remainder of a / b (truncated, taking the sign of the dividend).",
        remainder,
    );
    // `%quot` — truncating integer division (toward zero), the kernel `quot`
    // passes through to so the VM inlines it as one op. (It used to be Brood over
    // `(/ (- a (math/rem a b)) b)` — three dispatched calls per use, which made tight
    // integer loops like `collatz` pay rem+sub+div every step.)
    primitives.def(
        "%quot",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        &[],
        "",
        prim_quot,
    );
    // Ratio parts + conversions (exact rationals, ADR-196). `math/numerator`/`math/denominator`
    // accept an int (math/numerator = itself, denominator = 1) or a ratio.
    let int_or_ratio = int.union(ratio_ty);
    primitives.def(
        "math/numerator",
        Arity::exact(1),
        Sig::new(vec![int_or_ratio.clone()], int),
        &["x"],
        "The numerator of a ratio (`(math/numerator 3/4)` → 3), or an integer itself.",
        prim_numerator,
    );
    primitives.def(
        "math/denominator",
        Arity::exact(1),
        Sig::new(vec![int_or_ratio], int),
        &["x"],
        "The positive denominator of a ratio (`(math/denominator 3/4)` → 4), or 1 for an integer.",
        prim_denominator,
    );
    primitives.def(
        "decimal/number->",
        Arity::exact(1),
        Sig::new(vec![num], decimal_ty),
        &["x"],
        "A number as an exact base-10 decimal — exact for an integer or terminating ratio (`1/2` → `0.5M`); a non-terminating ratio rounds to the default precision.",
        prim_to_decimal);
    // `floor` is the single irreducible Float→Int crossing; ceil/round/pow/
    // sqrt are all Brood over it + rem/`/`/`*`/`<` (std/prelude.blsp).
    primitives.def(
        "math/floor",
        Arity::exact(1),
        Sig::new(vec![num], int),
        &["x"],
        "Round x toward negative infinity to an integer. Accepts the whole numeric tower; a ratio floors exactly (not through f64), so it stays correct past 2^53. Toward NEGATIVE infinity, so a negative argument rounds away from zero.\n\n    (math/floor 3.7)    → 3\n    (math/floor -3.2)   → -4",
        floor);
    // bitwise — integer bit-twiddling on the i64 two's-complement representation.
    // Table stakes for hashing, flags, and PRNGs (the std xorshift PRNG is built
    // on these); they were a noted gap (docs/feedback-retro-game-of-life.md).
    primitives.def(
        "bit/and",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        &["a", "b"],
        "Bitwise AND of integers a and b.\n\n    (bit/and 12 10)   → 8",
        bit_and,
    );
    primitives.def(
        "bit/or",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        &["a", "b"],
        "Bitwise (inclusive) OR of integers a and b.\n\n    (bit/or 12 10)   → 14",
        bit_or,
    );
    primitives.def(
        "bit/xor",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        &["a", "b"],
        "Bitwise exclusive-OR of integers a and b.\n\n    (bit/xor 12 10)   → 6",
        bit_xor,
    );
    primitives.def(
        "bit/not",
        Arity::exact(1),
        Sig::new(vec![int], int),
        &["a"],
        "Bitwise complement of integer a (two's-complement, so (bit/not n) = (- (- n) 1)).\n\n    (bit/not 12)   → -13",
        bit_not,
    );
    primitives.def(
        "bit/shift-left",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        &["a", "n"],
        "Shift integer a left by n bits (0 <= n < 64); bits shifted past bit 63 are discarded.\n\n    (bit/shift-left 12 10)   → 12288",
        bit_shift_left,
    );
    primitives.def(
        "bit/shift-right",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        &["a", "n"],
        "Arithmetic (sign-preserving) right shift of integer a by n bits (0 <= n < 64).\n\n    (bit/shift-right 12 10)   → 0",
        bit_shift_right,
    );
    primitives.def(
        "bit/count",
        Arity::exact(1),
        Sig::new(vec![int], int),
        &["a"],
        "Population count: the number of 1 bits in integer a's two's-complement representation (a negative a counts its sign bits, so (bit/count -1) = 64). For a bignum it is the popcount of the magnitude.\n\n    (bit/count 12)   → 2",
        bit_count);
    primitives.def(
        "bit/positions",
        Arity::exact(1),
        Sig::new(vec![int], vec_ty),
        &["a"],
        "A vector of the 0-based bit indices set in non-negative integer a, ascending (e.g. (bit/positions 6) = [1 2]). O(number of set bits) — for a bignum it scans the magnitude. The inverse of summing (bit/shift-left 1 i); handy for enumerating the set bits of an integer.\n\n    (bit/positions 12)   → [2 3]",
        bit_positions);
    // Bit-level reinterpretation of a binary64 — not expressible over the other
    // primitives (no bitcast, no frexp), and the only way to compare two floats
    // *exactly* (`-0.0` vs `0.0`, NaN payloads). Used by the conformance corpora.
    primitives.def(
        "bit/float->",
        Arity::exact(1),
        Sig::new(vec![num], int),
        &["x"],
        "The IEEE 754 binary64 bit pattern of x, as a non-negative integer (a bignum when the sign bit is set). Reinterpretation, not conversion — the only exact float comparison there is: it separates -0.0 from 0.0 and distinguishes NaN payloads, both of which = collapses. The inverse of bits->float.\n\n    (bit/float-> 12)   → 4622945017495814144",
        float_to_bits);
    primitives.def(
        "bit/->float",
        Arity::exact(1),
        Sig::new(vec![int], float),
        &["n"],
        "The binary64 float whose bit pattern is n (0 <= n < 2^64). The inverse of float->bits.\n\n    (bit/->float 12)   → 6e-323",
        bits_to_float,
    );
    // `decimal` constructs an exact base-10 decimal from a string ("1.50"), an
    // int (3), or a float (inexact source — uses its shortest round-trip form).
    primitives.def(
        "decimal/of",
        Arity::exact(1),
        Sig::new(vec![string.union(num)], decimal_ty),
        &["x"],
        "Construct an exact arbitrary-precision base-10 decimal from x: a string (\"1.50\"), an int (3), a bignum, or a float (converted from its shortest round-trip form, since a float is inexact). For money / Postgres numeric — values a float can't hold exactly. The literal form is a trailing M, e.g. 1.50M.\n\n    (decimal/of \"1.50\")   → 1.50M",
        prim_decimal);
    primitives.def(
        "decimal/->string",
        Arity::exact(1),
        Sig::new(vec![decimal_ty], string),
        &["d"],
        "The canonical decimal string of decimal d (no M suffix).\n\n    (decimal/->string (decimal/of \"1.50\"))   → \"1.50\"",
        prim_decimal_to_string,
    );
    primitives.def(
        "decimal/->float",
        Arity::exact(1),
        Sig::new(vec![decimal_ty], float),
        &["d"],
        "Decimal d as an (inexact) float.\n\n    (decimal/->float (decimal/of \"1.50\"))   → 1.5",
        prim_decimal_to_float,
    );
    primitives.def(
        "%atan2",
        Arity::exact(2),
        Sig::new(vec![num, num], float),
        &["y", "x"],
        "The angle in radians of the vector (x, y) from the positive x-axis, in (-π, π]. Handles x=0.",
        math_atan2);
    primitives.def(
        "%f64-sqrt",
        Arity::exact(1),
        Sig::new(vec![num], float),
        &["x"],
        "The IEEE 754 square root of x (f64::sqrt). x must be non-negative; raises otherwise. Handles subnormals and ±0 correctly. Any number coerces to f64 first — int, float, bignum, decimal or ratio.",
        math_f64_sqrt);
    // transcendental math — hardware f64 ops that can't be approximated in Brood
    // over `floor`/`rem`/`*` at the precision level scripts actually need.
    primitives.def(
        "%sin",
        Arity::exact(1),
        Sig::new(vec![num], float),
        &["x"],
        "The sine of x (radians). Returns a float.",
        math_sin,
    );
    primitives.def(
        "%cos",
        Arity::exact(1),
        Sig::new(vec![num], float),
        &["x"],
        "The cosine of x (radians). Returns a float.",
        math_cos,
    );
    primitives.def(
        "%tan",
        Arity::exact(1),
        Sig::new(vec![num], float),
        &["x"],
        "The tangent of x (radians). Returns a float.",
        math_tan,
    );
    primitives.def(
        "%asin",
        Arity::exact(1),
        Sig::new(vec![num], float),
        &["x"],
        "The arcsine of x in radians. x must be in [-1, 1]; raises otherwise.",
        math_asin,
    );
    primitives.def(
        "%acos",
        Arity::exact(1),
        Sig::new(vec![num], float),
        &["x"],
        "The arccosine of x in radians. x must be in [-1, 1]; raises otherwise.",
        math_acos,
    );
    primitives.def(
        "%atan",
        Arity::exact(1),
        Sig::new(vec![num], float),
        &["x"],
        "The arctangent of x in radians (result in [-π/2, π/2]).",
        math_atan,
    );
    primitives.def(
        "%exp",
        Arity::exact(1),
        Sig::new(vec![num], float),
        &["x"],
        "e raised to the power x. Returns a float.",
        math_exp,
    );
    primitives.def(
        "%ln",
        Arity::exact(1),
        Sig::new(vec![num], float),
        &["x"],
        "The natural logarithm of x. x must be positive; raises otherwise.",
        math_ln,
    );
    primitives.def(
        "%log2",
        Arity::exact(1),
        Sig::new(vec![num], float),
        &["x"],
        "The base-2 logarithm of x. x must be positive; raises otherwise.",
        math_log2,
    );
    primitives.def(
        "%log10",
        Arity::exact(1),
        Sig::new(vec![num], float),
        &["x"],
        "The base-10 logarithm of x. x must be positive; raises otherwise.",
        math_log10,
    );
    // string/->number returns int *or* float *or* nil (the parse-failed case).
    // The optional radix is what ADR-169 pointed at when it reserved `0x1F` as syntax
    // rather than a name. The literal itself exists now (ADR-334); this is how a radix
    // is read from text a program holds at runtime.
    primitives.def(
        "string/->number",
        Arity::range(1, 2),
        Sig::with_optional(vec![string], vec![int], num.union(Ty::of(Tag::Failure))),
        &["s", "&optional", "radix"],
        "Parse s strictly as an int (a bignum when out of i64 range), else a float, else a falsy failure value naming the input (unlike reflect/read-string). The inverse of str. With radix (2-36) the parse is integer-only in that base — how hex/octal/binary is read from DATA (in source, write the literal: 0xFF, 0b1010, 0o17); give the digits alone, no 0x/0b/0o prefix. A radix outside 2-36 raises.\n\n    (string/->number \"42\")   → 42\n    (string/->number \"1F\" 16)   → 31",
        string_to_number);
}

pub(crate) fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).copied().unwrap_or(Value::nil())
}

/// Destructure exactly two args. The declared `Arity` is the *primary* arity
/// check (enforced once in `eval::call_native` before any builtin runs); this
/// re-check is defense-in-depth for a direct Rust call that bypasses the gate
/// (e.g. a unit test) — it keeps such a call a clean error instead of a panic.
pub(super) fn two(args: &[Value], who: &str) -> Result<(Value, Value), LispError> {
    if args.len() != 2 {
        return Err(LispError::arity(format!(
            "{}: expected 2 arguments, got {}",
            who,
            args.len()
        )));
    }
    Ok((args[0], args[1]))
}

// ---------- numeric ----------

/// Require a number, coerced to `f64`; otherwise a self-identifying type error
/// attributed to `who` (the primitive that needed it).
pub(super) fn expect_number(heap: &Heap, who: &str, v: Value) -> Result<f64, LispError> {
    expect!(heap, who, v, "number",
        Value::Int(n) => n as f64,
        Value::Float(f) => f,
    )
}

/// Require a string, returned **owned** so the `heap` borrow is released before
/// the builtin reads or allocates further (most callers go on to touch
/// `&mut heap`). The string analogue of [`expect_int`]/[`expect_number`].
pub(super) fn expect_string(heap: &Heap, who: &str, v: Value) -> Result<String, LispError> {
    expect!(heap, who, v, "string",
        Value::Str(id) => heap.string(id).to_string(),
    )
}

/// Require a rope, returned **owned** (a cheap `Arc`-node clone) so the `heap`
/// borrow is released before the builtin edits or allocates a fresh rope.
pub(super) fn expect_rope(heap: &Heap, who: &str, v: Value) -> Result<ropey::Rope, LispError> {
    expect!(heap, who, v, "rope",
        Value::Rope(id) => heap.rope(id).clone(),
    )
}

/// Require a string, returned **borrowed** — no copy. The string sibling of
/// [`expect_rope_ref`], and for the same reason: [`expect_string`] hands back an owned
/// `String`, i.e. it **copies the whole string on every call**. That is invisible on a
/// short argument and quadratic on a long one — an incremental search over one big string
/// re-copied the haystack per probe, which dominated even after the char↔byte conversion
/// was made O(1). Use this for any builtin that only *reads* its string argument; reach
/// for `expect_string` only when an owned value is genuinely needed (because the heap is
/// mutated afterwards, or the value is moved into a fresh allocation).
pub(super) fn expect_string_ref<'h>(
    heap: &'h Heap,
    who: &str,
    v: Value,
) -> Result<crate::core::heap::SlabRef<'h, str>, LispError> {
    expect!(heap, who, v, "string",
        Value::Str(id) => heap.string(id),
    )
}

/// Require a rope, returned **borrowed** — no clone. For the read-only query
/// builtins (length/line/slice/…), all of ropey's queries take `&self`, so the
/// heap borrow can be held for the duration. Use this instead of `expect_rope`
/// whenever the rope isn't edited; it skips the per-call `Arc`-node clone on the
/// editor's hot path (viewport render calls `rope-line`/`rope-slice` per line per
/// frame). Reach for `expect_rope` only when allocating a fresh, edited rope.
pub(super) fn expect_rope_ref<'h>(
    heap: &'h Heap,
    who: &str,
    v: Value,
) -> Result<crate::core::heap::SlabRef<'h, ropey::Rope>, LispError> {
    expect!(heap, who, v, "rope",
        Value::Rope(id) => heap.rope(id),
    )
}

/// Require an integer; otherwise a self-identifying type error.
pub(super) fn expect_int(heap: &Heap, who: &str, v: Value) -> Result<i64, LispError> {
    expect!(heap, who, v, "int",
        Value::Int(n) => n,
    )
}

/// Require an integer (`Int` or `BigInt`), coerced to `num_bigint::BigInt`;
/// otherwise the standard self-identifying type error (which prints the offending
/// value). The bignum analogue of [`expect_int`] — `expect_int` rejects a
/// `BigInt`, but the bitwise / bignum-aware ops accept either, so they route
/// through here instead of losing the value to a bare `type_err`.
pub(super) fn expect_bigint(
    heap: &Heap,
    who: &str,
    v: Value,
) -> Result<num_bigint::BigInt, LispError> {
    heap.as_bigint(v)
        .ok_or_else(|| LispError::wrong_type(heap, who, "int", v))
}

/// Require a symbol; otherwise a self-identifying type error.
pub(super) fn expect_symbol(heap: &Heap, who: &str, v: Value) -> Result<value::Symbol, LispError> {
    expect!(heap, who, v, "symbol",
        Value::Sym(s) => s,
    )
}

/// True iff `v` is an integer (`Int` or `BigInt`) — the operand shape that
/// routes `+`/`-`/`*` through the bignum-promoting integer path rather than the
/// float path.
pub(super) fn is_integer(v: Value) -> bool {
    matches!(v, Value::Int(_) | Value::BigInt(_))
}

/// True iff `v` is a `Decimal`. A decimal is a number but **not** an integer, so
/// it routes through the exact-base-10 arithmetic path, not the bignum path.
pub(super) fn is_decimal(v: Value) -> bool {
    matches!(v, Value::Decimal(_))
}

/// True iff `v` is an exact `Ratio` (its own type — not an integer, since a
/// denominator of 1 is demoted to `Int` on construction).
pub(super) fn is_ratio(v: Value) -> bool {
    matches!(v, Value::Ratio(_))
}

/// Rationalizable-exact: `Int`/`BigInt`/`Ratio`/`Decimal` — everything the exact
/// ratio path can promote losslessly. A `Float` is excluded (it forces contagion).
fn is_rationalizable(v: Value) -> bool {
    is_integer(v) || is_ratio(v) || is_decimal(v)
}

/// The largest decimal **scale** (fractional-digit count; negative = trailing zeros)
/// any exact decimal path will materialise. Both the ratio conversion below and the
/// decimal arm of [`num_bin`] turn a scale into a `scale`-digit bignum — `10ˢᶜᵃˡᵉ`, or
/// the zero padding `with_scale` applies — so an unbounded scale is an unbounded
/// *native* allocation, which the ADR-043 `BROOD_MEM_LIMIT` cap never sees: measured,
/// `(+ 1/2 (decimal/of "1e-1000000000"))` sailed past 400MB and kept climbing with no
/// error and no end. A million digits is already far past any real decimal; beyond it
/// the operation raises a clean, catchable error.
const MAX_DEC_SCALE: i64 = 1_000_000;

/// The out-of-range-scale error shared by the exact decimal paths.
fn dec_scale_err(who: &str, scale: i64) -> LispError {
    LispError::runtime(format!(
        "{who}: decimal scale {scale} exceeds the maximum of ±{MAX_DEC_SCALE} \
         (the exact result would need a bignum with that many digits)"
    ))
}

/// Read an exact number as a `BigRational` — `Int`/`BigInt`/`Ratio` via
/// `as_bigrational`, and a `Decimal` losslessly (its value is `mantissa · 10⁻ˢᶜᵃˡᵉ`,
/// exactly `mantissa / 10ˢᶜᵃˡᵉ`). Panics on a non-rationalizable value (callers gate
/// with [`is_rationalizable`]).
///
/// Errors when the decimal's scale exceeds [`MAX_DEC_SCALE`]. The conversion is
/// `10^|scale|`, so the magnitude has to be checked *before* the `pow` — and the
/// exponent must be converted with `try_into`, never `as u32`: an `as` cast silently
/// truncated `4294967297` to `1`, so `(+ 1/2 (decimal/of "1e-4294967297"))` answered
/// `3/5` — a wrong answer with no diagnostic at all.
fn to_bigrational(
    heap: &Heap,
    who: &str,
    v: Value,
) -> Result<num_rational::BigRational, LispError> {
    use num_bigint::BigInt;
    if let Some(r) = heap.as_bigrational(v) {
        return Ok(r);
    }
    if let Value::Decimal(id) = v {
        let (m, scale) = heap.decimal(id).as_bigint_and_exponent();
        // `checked_abs` because `i64::MIN.abs()` itself overflows.
        let mag = scale.checked_abs().unwrap_or(i64::MAX);
        if mag > MAX_DEC_SCALE {
            return Err(dec_scale_err(who, scale));
        }
        let pow = BigInt::from(10).pow(u32::try_from(mag).map_err(|_| dec_scale_err(who, scale))?);
        return Ok(if scale >= 0 {
            num_rational::BigRational::new(m, pow)
        } else {
            num_rational::BigRational::from_integer(m * pow)
        });
    }
    unreachable!("to_bigrational on a non-rationalizable value")
}

/// Is this `BigRational` zero? (Its numerator is zero.) The exact-division
/// denominator check.
fn ratio_is_zero(r: &num_rational::BigRational) -> bool {
    use num_traits::Zero;
    r.is_zero()
}

/// True iff both operands are *exact* numbers (`Int`/`BigInt`/`Ratio`/`Decimal`) —
/// the shape `value_cmp` orders precisely (no f64 precision loss). A `Float` operand
/// is excluded so the comparison falls to the inexact f64 path.
fn both_exact(a: Value, b: Value) -> bool {
    is_rationalizable(a) && is_rationalizable(b)
}

/// Coerce an integer-or-float `Value` to `f64` for the float arithmetic path —
/// like [`expect_number`] but a `BigInt`/`Decimal` also coerces (via its
/// `to_f64`), so a mixed `(+ 2^200 1.5)` / `(+ 1.5M 2.0)` works rather than
/// rejecting it. A decimal coerced to f64 is the float-contagion path.
pub(super) fn num_to_f64(heap: &Heap, who: &str, v: Value) -> Result<f64, LispError> {
    use num_traits::ToPrimitive;
    match v {
        Value::BigInt(id) => Ok(heap.bigint(id).to_f64().unwrap_or(f64::INFINITY)),
        Value::Decimal(id) => {
            use bigdecimal::ToPrimitive as _;
            Ok(heap.decimal(id).to_f64().unwrap_or(f64::INFINITY))
        }
        // A ratio coerced to f64 is the float-contagion path (`(+ 1/2 1.5)`), and
        // the `->float` conversion.
        Value::Ratio(id) => Ok(heap.ratio(id).to_f64().unwrap_or(f64::INFINITY)),
        _ => expect_number(heap, who, v),
    }
}

/// The kernel of `+`/`-`/`*`. Two `Int`s try `int_op` (a `checked_*`) first and
/// stay an `Int` on success; on overflow — or when either operand is already a
/// `BigInt` — both operands promote to `num_bigint::BigInt`, `big_op` computes,
/// and the result demotes through [`Heap::int_from_bigint`] (so it comes back as
/// an `Int` whenever it fits). A float operand keeps the old `f64` path.
pub(super) fn num_bin(
    heap: &mut Heap,
    args: &[Value],
    who: &str,
    int_op: fn(i64, i64) -> Option<i64>,
    big_op: fn(num_bigint::BigInt, num_bigint::BigInt) -> num_bigint::BigInt,
    dec_op: fn(bigdecimal::BigDecimal, bigdecimal::BigDecimal) -> bigdecimal::BigDecimal,
    // `None` when the ideal exponent leaves i64 — `*` adds the operand scales, and
    // `(* (decimal/of "1e-5000000000000000000") …)` overflowed that add (a debug panic,
    // a wrapped scale in release).
    dec_scale: fn(i64, i64) -> Option<i64>,
    ratio_op: fn(num_rational::BigRational, num_rational::BigRational) -> num_rational::BigRational,
    float_op: fn(f64, f64) -> f64,
) -> LispResult {
    let (a, b) = two(args, who)?;
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => match int_op(x, y) {
            Some(r) => Ok(Value::int(r)),
            // Overflowed i64 — redo in BigInt and demote (route through the
            // normalizer for one code path; here the result is out of range).
            None => {
                let r = big_op(num_bigint::BigInt::from(x), num_bigint::BigInt::from(y));
                Ok(heap.int_from_bigint(r))
            }
        },
        // At least one BigInt, both integers: promote both, compute, demote.
        _ if is_integer(a) && is_integer(b) => {
            let x = heap.as_bigint(a).expect("integer");
            let y = heap.as_bigint(b).expect("integer");
            let r = big_op(x, y);
            Ok(heap.int_from_bigint(r))
        }
        // A ratio operand, and the other is rationalizable-exact (Int/BigInt/Ratio/
        // Decimal): compute exactly in `BigRational` and return a reduced `Ratio`
        // (demoted to `Int` when the denominator reduces to 1). A `Decimal` operand
        // promotes losslessly (so `(+ 1/2 0.5M)` is `1/1`). Checked before the decimal
        // arm so a ratio wins over a decimal; a `Float` operand falls to contagion.
        _ if (is_ratio(a) || is_ratio(b)) && is_rationalizable(a) && is_rationalizable(b) => {
            let x = to_bigrational(heap, who, a)?;
            let y = to_bigrational(heap, who, b)?;
            Ok(heap.alloc_ratio(ratio_op(x, y)))
        }
        // A decimal operand, and the other is exact (Int/BigInt/Decimal): compute
        // exactly in BigDecimal and return a `Decimal`. (A Float operand falls
        // through to the float-contagion path below — float wins, inexact.)
        _ if (is_decimal(a) || is_decimal(b))
            && (is_integer(a) || is_decimal(a))
            && (is_integer(b) || is_decimal(b)) =>
        {
            let x = heap.as_bigdecimal(a).expect("exact number");
            let y = heap.as_bigdecimal(b).expect("exact number");
            // Pin the result to the standard's **ideal exponent** (`dec_scale`):
            // max(sx, sy) for +/-, sx+sy for *. `bigdecimal` computes the right
            // VALUE but not always the right scale — its `Sub` short-circuits on a
            // zero operand and its `Mul` on a one-valued operand, each returning
            // the other operand verbatim and so discarding the short-circuited
            // side's scale (`1 - 0.0` -> `1`, `1.00 * -1` -> `-1`). Significance is
            // the whole point of a decimal: money scaled by `1.00M` must stay in
            // cents. The exact result never needs MORE than the ideal scale, so
            // `with_scale` only ever pads with zeros — it cannot round here.
            // (Found by the dectest corpus; see tests/conformance_dectest_test.blsp.)
            //
            // The scale is validated BEFORE `dec_op` runs: both the operation and the
            // `with_scale` padding materialise that many digits, so an out-of-range
            // ideal exponent has to become a clean error rather than an unbounded
            // native allocation (or, for `*`, a wrapped i64 scale) — see MAX_DEC_SCALE.
            let (sx, sy) = (x.fractional_digit_count(), y.fractional_digit_count());
            let scale = match dec_scale(sx, sy) {
                Some(s) if s.checked_abs().unwrap_or(i64::MAX) <= MAX_DEC_SCALE => s,
                // Report the offending input scale when the ideal exponent overflowed.
                Some(s) => return Err(dec_scale_err(who, s)),
                None => return Err(dec_scale_err(who, sx.max(sy))),
            };
            Ok(heap.alloc_decimal(dec_op(x, y).with_scale(scale)))
        }
        // A record operand (a map with a truthy __id__) routes to the `Num` multimethod,
        // dispatching on both operands. Gating on the Map *tag* keeps the record check off
        // the numeric path — a float/decimal operand falls straight to the float arm. A pair
        // with no method raises `no-method`; a plain (non-record) map errors in `num_to_f64`.
        (Value::Map(_), _) | (_, Value::Map(_)) if is_record(heap, a) || is_record(heap, b) => {
            num_multi_dispatch(heap, who, a, b)
        }
        // A float operand anywhere: the float path (a BigInt/Decimal coerces via `f64` —
        // float contagion, like Clojure's double contagion). A plain non-numeric operand
        // still errors in `num_to_f64`.
        _ => Ok(Value::Float(float_op(
            num_to_f64(heap, who, a)?,
            num_to_f64(heap, who, b)?,
        ))),
    }
}

/// True when `v` is an identity-carrying record — a map whose reserved `:__id__` key
/// (`defrecord` bakes it in) holds a *truthy* id. A plain map, or a hand-written
/// `{:__id__ nil}`, is not a record: this matches `record?`/`identity-of`, which
/// dispatch a nil/false id as `:map` (devlog 2026-07-29).
fn is_record(heap: &Heap, v: Value) -> bool {
    let Value::Map(m) = v else { return false };
    match heap.map_get(m, Value::Keyword(crate::core::value::intern("__id__"))) {
        Some(id) => !matches!(id, Value::Nil | Value::Bool(false)),
        None => false,
    }
}

/// Dispatch an arithmetic operator to the `Num` MULTIMETHOD (ADR-179) when a record is an
/// operand: `+`/`-`/`*`/`/` → `num/add`/`num/sub`/`num/mul`/`num/div`, each dispatching on
/// BOTH operand identities. Reached ONLY on the cold fallback — the JIT/VM inlines int+int
/// and float+float and never calls `%add` for them, so the numeric hot path is byte-for-byte
/// untouched (a Brood `(record? a)` branch in `+` measured ~195×). A pair with no method
/// raises the multimethod's loud `no-method` error, so mixed types are explicit, never
/// silently coerced.
fn num_multi_dispatch(heap: &mut Heap, who: &str, a: Value, b: Value) -> LispResult {
    let op = match who {
        "+" => "num/add",
        "-" => "num/sub",
        "*" => "num/mul",
        "/" => "num/div",
        _ => unreachable!("num_multi_dispatch: {who} is not a Num operator"),
    };
    let genv = heap.global();
    let callee = heap
        .env_get(genv, crate::core::value::intern(op))
        .ok_or_else(|| {
            LispError::runtime(format!("{who}: the `{op}` multimethod is not loaded"))
        })?;
    crate::eval::compile::apply_value(heap, callee, &[a, b], genv)
}

/// Dispatch a comparison operator to the `Ord` MULTIMETHOD (`compare-to`, ADR-179) when a
/// record is an operand, returning the sign as an `Ordering`. `compare-to` dispatches on both
/// operands and returns an int (<0 before, 0 equal, >0 after). Cold path only — pure numeric
/// comparison never reaches here.
fn compare_multi_dispatch(
    heap: &mut Heap,
    who: &str,
    a: Value,
    b: Value,
) -> Result<std::cmp::Ordering, LispError> {
    let genv = heap.global();
    let callee = heap
        .env_get(genv, crate::core::value::intern("compare-to"))
        .ok_or_else(|| {
            LispError::runtime(format!("{who}: the `compare-to` multimethod is not loaded"))
        })?;
    match crate::eval::compile::apply_value(heap, callee, &[a, b], genv)? {
        Value::Int(n) => Ok(n.cmp(&0)),
        _ => Err(LispError::runtime(format!(
            "{who}: `compare-to` must return an int (-1/0/1)"
        ))),
    }
}

pub(super) fn prim_add(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    num_bin(
        heap,
        args,
        "+",
        i64::checked_add,
        |a, b| a + b,
        |a, b| a + b,
        // ideal exponent of a sum: the finer of the two scales (never overflows)
        |sa, sb| Some(sa.max(sb)),
        |a, b| a + b,
        |a, b| a + b,
    )
}
pub(super) fn prim_sub(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    num_bin(
        heap,
        args,
        "-",
        i64::checked_sub,
        |a, b| a - b,
        |a, b| a - b,
        |sa, sb| Some(sa.max(sb)),
        |a, b| a - b,
        |a, b| a - b,
    )
}
pub(super) fn prim_mul(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    num_bin(
        heap,
        args,
        "*",
        i64::checked_mul,
        |a, b| a * b,
        |a, b| a * b,
        // ideal exponent of a product: the scales add (checked — this one can overflow)
        |sa, sb| sa.checked_add(sb),
        |a, b| a * b,
        |a, b| a * b,
    )
}

pub(super) fn prim_div(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "/")?;
    // A record operand (either position) dispatches the `Num` multimethod (`num/div`, on both
    // operands); the `is_record` check is cheap, and int/float division is JIT-inlined and
    // never reaches here.
    if is_record(heap, a) || is_record(heap, b) {
        return num_multi_dispatch(heap, "/", a, b);
    }
    // ---- Exact rational division (ADR-196) ----
    // `/` on two integers is EXACT: `(/ 6 3)` → `2` (an Int, divides evenly), but
    // `(/ 1 2)` → `1/2` (a reduced Ratio) rather than a float. Likewise any division
    // involving a `Ratio` (with the other operand rationalizable — Int/BigInt/Ratio/
    // Decimal, a Decimal promoting losslessly) is exact. `alloc_ratio` demotes a
    // denominator of 1 back to an integer. Reach for `->float` for an inexact result.
    if (is_integer(a) && is_integer(b))
        || ((is_ratio(a) || is_ratio(b)) && is_rationalizable(a) && is_rationalizable(b))
    {
        let y = to_bigrational(heap, "/", b)?;
        if ratio_is_zero(&y) {
            return Err(div_by_zero());
        }
        let x = to_bigrational(heap, "/", a)?;
        return Ok(heap.alloc_ratio(x / y));
    }
    // ---- Exact decimal division (a decimal operand, no ratio; both exact) ----
    if (is_decimal(a) || is_decimal(b))
        && (is_integer(a) || is_decimal(a))
        && (is_integer(b) || is_decimal(b))
    {
        let y = heap.as_bigdecimal(b).expect("exact number");
        if num_traits::Zero::is_zero(&y) {
            return Err(div_by_zero());
        }
        let x = heap.as_bigdecimal(a).expect("exact number");
        return Ok(heap.alloc_decimal(x / y));
    }
    // ---- Float contagion ----
    let bf = num_to_f64(heap, "/", b)?;
    if bf == 0.0 {
        return Err(div_by_zero());
    }
    Ok(Value::Float(num_to_f64(heap, "/", a)? / bf))
}

/// `(math/numerator x)` — the numerator of a ratio, or an integer itself (its
/// numerator over 1). Errors on a non-rational number.
pub(super) fn prim_numerator(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let x = arg(args, 0);
    match x {
        Value::Ratio(id) => {
            let n = heap.ratio(id).numer().clone();
            Ok(heap.int_from_bigint(n))
        }
        Value::Int(_) | Value::BigInt(_) => Ok(x),
        _ => Err(LispError::wrong_type(
            heap,
            "math/numerator",
            "int or ratio",
            x,
        )),
    }
}

/// `(math/denominator x)` — the (positive) denominator of a ratio, or `1` for an integer.
pub(super) fn prim_denominator(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let x = arg(args, 0);
    match x {
        Value::Ratio(id) => {
            let d = heap.ratio(id).denom().clone();
            Ok(heap.int_from_bigint(d))
        }
        Value::Int(_) | Value::BigInt(_) => Ok(Value::int(1)),
        _ => Err(LispError::wrong_type(
            heap,
            "math/denominator",
            "int or ratio",
            x,
        )),
    }
}

/// `(decimal/number-> x)` — a number as an exact base-10 `Decimal`. Exact for an integer or
/// a terminating ratio (`1/2` → `0.5M`); a non-terminating ratio (`1/3`) rounds to
/// `bigdecimal`'s default precision. A `Float` coerces through its decimal form.
pub(super) fn prim_to_decimal(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use bigdecimal::BigDecimal;
    let x = arg(args, 0);
    match x {
        Value::Decimal(_) => Ok(x),
        Value::Int(i) => Ok(heap.alloc_decimal(BigDecimal::from(i))),
        Value::BigInt(id) => {
            let n = heap.bigint(id).clone();
            Ok(heap.alloc_decimal(BigDecimal::from(n)))
        }
        Value::Ratio(id) => {
            let r = heap.ratio(id).clone();
            let d = BigDecimal::from(r.numer().clone()) / BigDecimal::from(r.denom().clone());
            Ok(heap.alloc_decimal(d))
        }
        Value::Float(f) => match BigDecimal::try_from(f) {
            Ok(d) => Ok(heap.alloc_decimal(d)),
            Err(_) => Err(LispError::runtime(
                "decimal/number->: cannot convert a non-finite float",
            )),
        },
        _ => Err(LispError::wrong_type(heap, "decimal/number->", "number", x)),
    }
}

/// The shared `division by zero` error (`/` raises rather than returning IEEE ∞).
fn div_by_zero() -> LispError {
    LispError::runtime("division by zero")
        .with_code(crate::error::error_codes::DIV_BY_ZERO)
        .with_hint("guard the denominator: (when (not (= y 0)) (/ x y))")
}

pub(super) fn prim_lt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "<")?;
    // Compare two integers directly; coercing to f64 first loses precision past
    // 2^53 (e.g. `(< 9007199254740992 9007199254740993)` would wrongly be false).
    // `value_cmp` already handles Int/BigInt exactly and the mixed int/float and
    // BigInt/float cases.
    let lt = match (a, b) {
        (Value::Int(x), Value::Int(y)) => x < y,
        _ if both_exact(a, b) => heap.value_cmp(a, b) == std::cmp::Ordering::Less,
        // A record operand (a map with a truthy __id__) routes to the `Ord` multimethod.
        // Gating on the Map *tag* in the pattern keeps the record check off the numeric
        // path — a float/decimal operand never reaches the `is_record` guard below.
        (Value::Map(_), _) | (_, Value::Map(_)) if is_record(heap, a) || is_record(heap, b) => {
            return Ok(Value::boolean(
                compare_multi_dispatch(heap, "<", a, b)? == std::cmp::Ordering::Less,
            ));
        }
        _ => num_to_f64(heap, "<", a)? < num_to_f64(heap, "<", b)?,
    };
    Ok(Value::boolean(lt))
}

/// `(%le a b)` — `a <= b`. The `<=`/`>=` kernel: a direct primitive so the 2-arg
/// clauses of `<=`/`>=` are pure passthroughs the ADR-069 thin-wrapper elision can
/// reach (the old `(not (%lt …))` bodies were a nested call it couldn't). Same
/// int-exact / float-coerce care as `%lt`.
pub(super) fn prim_le(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "<=")?;
    let le = match (a, b) {
        (Value::Int(x), Value::Int(y)) => x <= y,
        _ if both_exact(a, b) => heap.value_cmp(a, b) != std::cmp::Ordering::Greater,
        // Map-tagged record operand → the `Ord` multimethod; the numeric path stays pure.
        (Value::Map(_), _) | (_, Value::Map(_)) if is_record(heap, a) || is_record(heap, b) => {
            return Ok(Value::boolean(
                compare_multi_dispatch(heap, "<=", a, b)? != std::cmp::Ordering::Greater,
            ));
        }
        _ => num_to_f64(heap, "<=", a)? <= num_to_f64(heap, "<=", b)?,
    };
    Ok(Value::boolean(le))
}

pub(super) fn prim_max(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut best = args[0]; // Arity::at_least(1) ensures non-empty
    for &v in &args[1..] {
        let replace = match (best, v) {
            (Value::Int(a), Value::Int(b)) => b > a,
            _ if both_exact(best, v) => heap.value_cmp(best, v) == std::cmp::Ordering::Less,
            // record (Map-tagged): keep the larger — replace when best sorts before v.
            (Value::Map(_), _) | (_, Value::Map(_))
                if is_record(heap, best) || is_record(heap, v) =>
            {
                compare_multi_dispatch(heap, "max", best, v)? == std::cmp::Ordering::Less
            }
            _ => num_to_f64(heap, "max", v)? > num_to_f64(heap, "max", best)?,
        };
        if replace {
            best = v;
        }
    }
    Ok(best)
}

pub(super) fn prim_min(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut best = args[0]; // Arity::at_least(1) ensures non-empty
    for &v in &args[1..] {
        let replace = match (best, v) {
            (Value::Int(a), Value::Int(b)) => b < a,
            _ if both_exact(best, v) => heap.value_cmp(best, v) == std::cmp::Ordering::Greater,
            // record (Map-tagged): keep the smaller — replace when best sorts after v.
            (Value::Map(_), _) | (_, Value::Map(_))
                if is_record(heap, best) || is_record(heap, v) =>
            {
                compare_multi_dispatch(heap, "min", best, v)? == std::cmp::Ordering::Greater
            }
            _ => num_to_f64(heap, "min", v)? < num_to_f64(heap, "min", best)?,
        };
        if replace {
            best = v;
        }
    }
    Ok(best)
}

pub(super) fn prim_eq(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, kw::EQ_PRIM)?;
    // Fast path: no lazy seq-view operand — the overwhelming common case (and the
    // only one the inlined `Eq` ever defers here for non-ints). Scalar equality
    // pays nothing.
    if !matches!(a, Value::SeqView(_)) && !matches!(b, Value::SeqView(_)) {
        return Ok(Value::boolean(heap.equal(a, b)));
    }
    // A view compares structurally as the list it stands in for — realise it (the
    // kernel `equal` can't run a transducer). Root both operands across each
    // realise, since `apply` can collect and move the other handle.
    heap.root_scope(|heap| {
        let a_r = heap.root(a);
        let b_r = heap.root(b);
        let a = heap.read_root(a_r);
        let a = if matches!(a, Value::SeqView(_)) {
            realize_seqview(heap, env, a)?
        } else {
            a
        };
        let a_r = heap.root(a);
        let b = heap.read_root(b_r);
        let b = if matches!(b, Value::SeqView(_)) {
            realize_seqview(heap, env, b)?
        } else {
            b
        };
        let a = heap.read_root(a_r);
        Ok(Value::boolean(heap.equal(a, b)))
    })
}

/// Read two arguments as `num_bigint::BigInt`s (`Int`s promote), for the
/// bignum-aware integer ops (`rem`/`quot`/the bitwise family). A self-identifying
/// type error if either isn't an integer.
pub(super) fn bigint_pair(
    heap: &Heap,
    args: &[Value],
    who: &str,
) -> Result<(num_bigint::BigInt, num_bigint::BigInt), LispError> {
    let (a, b) = two(args, who)?;
    let x = expect_bigint(heap, who, a)?;
    let y = expect_bigint(heap, who, b)?;
    Ok((x, y))
}

pub(super) fn remainder(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "rem")?;
    // i64 fast path. `checked_rem` returns None on `b == 0` (div-by-zero) and
    // on the lone `i64::MIN % -1` overflow — that overflow is mathematically 0,
    // so handle it directly rather than promoting.
    if let (Value::Int(x), Value::Int(y)) = (a, b) {
        return match x.checked_rem(y) {
            Some(r) => Ok(Value::int(r)),
            None if y == 0 => Err(LispError::runtime("rem: division by zero")
                .with_code(crate::error::error_codes::DIV_BY_ZERO)
                .with_hint("guard the denominator: (when (not (= y 0)) (math/rem x y))")),
            None => Ok(Value::int(0)), // i64::MIN % -1
        };
    }
    let (x, y) = bigint_pair(heap, args, "rem")?;
    if num_traits::Zero::is_zero(&y) {
        return Err(LispError::runtime("rem: division by zero")
            .with_code(crate::error::error_codes::DIV_BY_ZERO)
            .with_hint("guard the denominator: (when (not (= y 0)) (math/rem x y))"));
    }
    // `BigInt::%` truncates toward zero (matches i64 `%`), so the remainder has
    // the dividend's sign — the non-Euclidean `rem` the prelude `mod` builds on.
    Ok(heap.int_from_bigint(x % y))
}

/// `(%quot a b)` — truncating integer division toward zero, the kernel `quot`
/// passes through to. `checked_div` truncates toward zero (matching the old
/// `(/ (- a (math/rem a b)) b)` integer result) and guards both `b == 0` and the lone
/// `i64::MIN / -1` overflow; that overflow promotes to BigInt.
pub(super) fn prim_quot(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "quot")?;
    if let (Value::Int(x), Value::Int(y)) = (a, b) {
        match x.checked_div(y) {
            Some(q) => return Ok(Value::int(q)),
            None if y == 0 => {
                return Err(LispError::runtime("quot: division by zero")
                    .with_code(crate::error::error_codes::DIV_BY_ZERO)
                    .with_hint("guard the denominator: (when (not (= y 0)) (math/quot x y))"))
            }
            None => {} // i64::MIN / -1 — promote and fall through
        }
    }
    let (x, y) = bigint_pair(heap, args, "quot")?;
    if num_traits::Zero::is_zero(&y) {
        return Err(LispError::runtime("quot: division by zero")
            .with_code(crate::error::error_codes::DIV_BY_ZERO)
            .with_hint("guard the denominator: (when (not (= y 0)) (math/quot x y))"));
    }
    // `BigInt::/` truncates toward zero, matching i64 `checked_div`.
    Ok(heap.int_from_bigint(x / y))
}

/// Floor toward negative infinity, returning an `Int` — the one Float→Int
/// crossing the language can't bootstrap (no other primitive produces an `Int`
/// from a `Float`). An `Int` passes through; a `Float` is floored. `NaN` and
/// values whose floor doesn't fit in `i64` are runtime errors — pre-fix the
/// `as i64` cast silently saturated, so `(math/floor (* 1e308 1e308))` returned
/// `i64::MAX` and `(math/floor (/ 0.0 0.0))` returned `0`. `ceil`/`round`/`quot`/
/// `pow`/`sqrt` are all Brood over this + `rem`/`/`/`*`/`<` (std/prelude.blsp).
pub(super) fn floor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Int(n) => Ok(Value::int(n)),
        // A bignum is already an integer — it is its own floor.
        v @ Value::BigInt(_) => Ok(v),
        // A ratio floors **exactly**, not through f64. Since ADR-196 made `/` on
        // integers exact, `(math/floor (/ a b))` is an ordinary idiom, and routing it
        // through f64 would round the answer for ratios beyond 2^53 — a wrong
        // integer, not merely an imprecise one.
        Value::Ratio(id) => {
            let r = heap.ratio(id).clone();
            Ok(heap.int_from_bigint(r.floor().to_integer()))
        }
        v => {
            let f = num_to_f64(heap, "floor", v)?.floor();
            if !f.is_finite() {
                return Err(LispError::runtime(format!(
                    "floor: argument {} has no integer floor",
                    f
                ))
                .with_code(crate::error::error_codes::INT_OVERFLOW));
            }
            // `i64::MIN as f64` rounds *down* to a value still in range; the
            // upper bound `i64::MAX as f64` rounds *up* past `i64::MAX`, so
            // the open upper comparison is the right one.
            if f < i64::MIN as f64 || f >= i64::MAX as f64 + 1.0 {
                return Err(
                    LispError::runtime(format!("floor: {} is out of range for i64", f))
                        .with_code(crate::error::error_codes::INT_OVERFLOW),
                );
            }
            Ok(Value::int(f as i64))
        }
    }
}

// ---------- bitwise ----------

pub(super) fn bit_and(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if let (Value::Int(a), Value::Int(b)) = (arg(args, 0), arg(args, 1)) {
        return Ok(Value::int(a & b));
    }
    // num-bigint implements bitwise ops on its (infinite) two's-complement
    // model, so this matches the i64 result on small values and extends it.
    let (a, b) = bigint_pair(heap, args, "bit/and")?;
    Ok(heap.int_from_bigint(a & b))
}

pub(super) fn bit_or(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if let (Value::Int(a), Value::Int(b)) = (arg(args, 0), arg(args, 1)) {
        return Ok(Value::int(a | b));
    }
    let (a, b) = bigint_pair(heap, args, "bit/or")?;
    Ok(heap.int_from_bigint(a | b))
}

pub(super) fn bit_xor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if let (Value::Int(a), Value::Int(b)) = (arg(args, 0), arg(args, 1)) {
        return Ok(Value::int(a ^ b));
    }
    let (a, b) = bigint_pair(heap, args, "bit/xor")?;
    Ok(heap.int_from_bigint(a ^ b))
}

pub(super) fn bit_not(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Int(n) => Ok(Value::int(!n)),
        Value::BigInt(id) => {
            let n = !heap.bigint(id).clone();
            Ok(heap.int_from_bigint(n))
        }
        v => Err(LispError::wrong_type(heap, "bit/not", "int", v)),
    }
}

pub(super) fn bit_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Int(n) => Ok(Value::int(i64::from(n.count_ones()))),
        // Popcount of the MAGNITUDE (abs value) — the bitboard only uses
        // non-negative values, so we count the set bits of |n| (`BigUint`'s
        // `count_ones`), sign-independent.
        Value::BigInt(id) => {
            let bits = heap.bigint(id).magnitude().count_ones();
            Ok(Value::int(bits as i64))
        }
        v => Err(LispError::wrong_type(heap, "bit/count", "int", v)),
    }
}

pub(super) fn bit_positions(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // The 0-based indices of the set bits, ascending. O(popcount): pull the
    // lowest set bit, record it, clear it, repeat — so enumerating a sparsely
    // populated integer costs the number of set bits, not the bit width.
    let mut out: Vec<Value> = Vec::new();
    match arg(args, 0) {
        Value::Int(n) => {
            let mut bits = n as u64; // the two's-complement bit pattern (bitboard words are non-negative)
            while bits != 0 {
                out.push(Value::int(i64::from(bits.trailing_zeros())));
                bits &= bits - 1; // clear the lowest set bit
            }
        }
        Value::BigInt(id) => {
            let mut mag = heap.bigint(id).magnitude().clone();
            while let Some(i) = mag.trailing_zeros() {
                out.push(Value::int(i as i64));
                mag.set_bit(i, false);
            }
        }
        v => return Err(LispError::wrong_type(heap, "bit/positions", "int", v)),
    }
    Ok(heap.alloc_vector(out))
}

/// `(bit/float-> x)` — the IEEE 754 binary64 bit pattern of `x` as a non-negative
/// integer (a bignum whenever the sign bit is set, since the pattern is a *u64*).
/// Reinterpretation, not conversion: the only way to compare two floats *exactly*,
/// including distinguishing `-0.0` from `0.0` and the individual NaN payloads that
/// `=` collapses. Its inverse is `bit/->float`.
pub(super) fn float_to_bits(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let f = num_to_f64(heap, "bit/float->", arg(args, 0))?;
    Ok(heap.int_from_bigint(num_bigint::BigInt::from(f.to_bits())))
}

/// `(bit/->float n)` — the binary64 float with bit pattern `n`. The inverse of
/// `bit/float->`; `n` must be in `[0, 2^64)`.
pub(super) fn bits_to_float(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use num_traits::ToPrimitive;
    let bits = match arg(args, 0) {
        Value::Int(n) => n.to_u64(),
        Value::BigInt(id) => heap.bigint(id).to_u64(),
        v => return Err(LispError::wrong_type(heap, "bit/->float", "int", v)),
    };
    match bits {
        Some(b) => Ok(Value::float(f64::from_bits(b))),
        // Out of u64 range in either direction — negative, or wider than 64 bits.
        None => Err(LispError::runtime(
            "bit/->float: bit pattern out of range (must be 0 <= n < 2^64)".to_string(),
        )),
    }
}

/// Validate a shift amount: non-negative (a negative shift is an error) and not
/// absurdly large (cap well above any realistic bit width so a typo'd
/// `(bit/shift-left 1 1e9)` can't try to allocate gigabytes). Returns the amount
/// as `usize`. No upper *bit-width* cap any more — large shifts promote to
/// BigInt (the whole point of the bitboard use).
pub(super) fn shift_amount(n: i64, who: &str) -> Result<usize, LispError> {
    if n < 0 {
        return Err(LispError::runtime(format!(
            "{}: negative shift amount {}",
            who, n
        )));
    }
    // ~128 Mbit: far past any legitimate use, but bounds the worst-case alloc.
    const MAX_SHIFT: i64 = 1 << 27;
    if n > MAX_SHIFT {
        return Err(LispError::runtime(format!(
            "{}: shift amount {} too large (math/max {})",
            who, n, MAX_SHIFT
        )));
    }
    Ok(n as usize)
}

pub(super) fn bit_shift_left(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let a = arg(args, 0);
    let n = expect_int(heap, "bit/shift-left", arg(args, 1))?;
    let amount = shift_amount(n, "bit/shift-left")?;
    // i64 fast path: stay an `Int` when the shift fits, else promote. (Unlike the
    // old wrapping shift, an i64 result that would lose bits past the top now
    // promotes to BigInt — the conventional arbitrary-width left shift.)
    if let Value::Int(x) = a {
        if amount < 64 {
            if let Some(r) = x.checked_shl(amount as u32) {
                // checked_shl only guards the *shift amount*, not value overflow;
                // verify the shift is lossless before keeping the i64 result.
                if (r >> amount) == x {
                    return Ok(Value::int(r));
                }
            }
        }
    }
    let x = expect_bigint(heap, "bit/shift-left", a)?;
    Ok(heap.int_from_bigint(x << amount))
}

pub(super) fn bit_shift_right(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let a = arg(args, 0);
    let n = expect_int(heap, "bit/shift-right", arg(args, 1))?;
    let amount = shift_amount(n, "bit/shift-right")?;
    // Arithmetic (sign-preserving) right shift, matching the signed model.
    if let Value::Int(x) = a {
        // A right shift ≥ 64 collapses to the sign bit (0 or -1).
        let r = if amount >= 64 { x >> 63 } else { x >> amount };
        return Ok(Value::int(r));
    }
    let x = expect_bigint(heap, "bit/shift-right", a)?;
    Ok(heap.int_from_bigint(x >> amount))
}

// ---------- decimal ----------

/// `(decimal/of x)` — construct an exact base-10 `Decimal` from a string ("1.50"),
/// an int (3), or a float. A string parses exactly; an int is exact; a float is
/// converted from its *shortest round-trip* decimal text (an f64 is inexact, so
/// `(decimal/of 0.1)` is the decimal `0.1`, the value the literal `0.1` denotes,
/// not the full binary expansion). A `BigInt` is also accepted (exact).
pub(super) fn prim_decimal(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::str::FromStr;
    let v = arg(args, 0);
    let d = match v {
        // Already a decimal — return it unchanged.
        Value::Decimal(_) => return Ok(v),
        Value::Int(n) => bigdecimal::BigDecimal::from(n),
        Value::BigInt(id) => bigdecimal::BigDecimal::from(heap.bigint(id).clone()),
        Value::Float(f) => {
            if !f.is_finite() {
                return Err(LispError::runtime(format!(
                    "decimal: cannot represent non-finite float {f}"
                )));
            }
            // Shortest round-trip text (what the printer would show), parsed exactly.
            bigdecimal::BigDecimal::from_str(&format_decimal_from_float(f))
                .map_err(|_| LispError::runtime(format!("decimal: cannot convert float {f}")))?
        }
        Value::Str(id) => {
            let s = heap.string(id).trim().to_string();
            bigdecimal::BigDecimal::from_str(&s).map_err(|_| {
                LispError::runtime(format!("decimal: malformed decimal string {s:?}"))
            })?
        }
        other => {
            return Err(LispError::wrong_type(
                heap,
                "decimal",
                "string or number",
                other,
            ))
        }
    };
    Ok(heap.alloc_decimal(d))
}

/// Render an f64 as the shortest decimal text that round-trips back to it — the
/// same form the value printer uses, so `(decimal/of 1.5)` is `1.5M`, not the long
/// binary expansion of the nearest f64.
fn format_decimal_from_float(f: f64) -> String {
    // `{}` on f64 already prints the shortest round-trip representation in Rust.
    format!("{f}")
}

/// `(decimal/->string d)` — the canonical decimal string of `d` (no `M` suffix).
pub(super) fn prim_decimal_to_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let s = match v {
        Value::Decimal(id) => heap.decimal(id).to_string(),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "decimal/->string",
                "decimal",
                other,
            ))
        }
    };
    Ok(heap.alloc_string(&s))
}

/// `(decimal/->float d)` — `d` as an (inexact) `f64`.
pub(super) fn prim_decimal_to_float(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use bigdecimal::ToPrimitive;
    let v = arg(args, 0);
    match v {
        Value::Decimal(id) => Ok(Value::float(
            heap.decimal(id).to_f64().unwrap_or(f64::INFINITY),
        )),
        other => Err(LispError::wrong_type(
            heap,
            "decimal/->float",
            "decimal",
            other,
        )),
    }
}

// ---------- transcendental math ----------

macro_rules! math1_unrestricted {
    ($name:ident, $brood:literal, $method:ident) => {
        pub(super) fn $name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
            let x = num_to_f64(heap, $brood, arg(args, 0))?;
            Ok(Value::float(x.$method()))
        }
    };
}

macro_rules! math1_bounded {
    ($name:ident, $brood:literal, $method:ident) => {
        pub(super) fn $name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
            let x = num_to_f64(heap, $brood, arg(args, 0))?;
            if x < -1.0 || x > 1.0 {
                return Err(LispError::runtime(format!(
                    "{}: argument {} is out of domain [-1, 1]",
                    $brood, x
                )));
            }
            Ok(Value::float(x.$method()))
        }
    };
}

macro_rules! math1_positive {
    ($name:ident, $brood:literal, $method:ident) => {
        pub(super) fn $name(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
            let x = num_to_f64(heap, $brood, arg(args, 0))?;
            if x <= 0.0 {
                return Err(LispError::runtime(format!(
                    "{}: argument {} must be positive",
                    $brood, x
                )));
            }
            Ok(Value::float(x.$method()))
        }
    };
}

math1_unrestricted!(math_sin, "%sin", sin);
math1_unrestricted!(math_cos, "%cos", cos);
math1_unrestricted!(math_tan, "%tan", tan);
math1_unrestricted!(math_atan, "%atan", atan);
math1_unrestricted!(math_exp, "%exp", exp);
math1_bounded!(math_asin, "%asin", asin);
math1_bounded!(math_acos, "%acos", acos);
math1_positive!(math_ln, "%ln", ln);
math1_positive!(math_log2, "%log2", log2);
math1_positive!(math_log10, "%log10", log10);

pub(super) fn math_f64_sqrt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let x = num_to_f64(heap, "%f64-sqrt", arg(args, 0))?;
    if x < 0.0 {
        return Err(LispError::runtime(format!(
            "%f64-sqrt: argument {} must be non-negative",
            x
        )));
    }
    Ok(Value::float(x.sqrt()))
}

pub(super) fn math_atan2(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let y = num_to_f64(heap, "%atan2", arg(args, 0))?;
    let x = num_to_f64(heap, "%atan2", arg(args, 1))?;
    Ok(Value::float(y.atan2(x)))
}

/// `(string/->number s &optional radix)` — parse `s` as an integer if it is one, else as
/// a float, else a **failure** value naming the input (falsy, so `(or … d)` still
/// defaults, but it says *why* where `nil` could not). The inverse of `str`. A robust parse can't be
/// expressed over `reflect/read-string` (which would read `"3abc"` as `3` and stop), so
/// the strict parse is a primitive. Surrounding whitespace is not accepted —
/// `trim` first if the input may carry any.
///
/// **`radix` is how hex, octal or binary is read from DATA.** ADR-169 reserved
/// `0x1F`/`0b1010`/`0o17` as syntax rather than names *on the grounds that this function
/// covers the need at runtime* — and the reader's own hint, `syntax/atom.rs`'s
/// `reserved_numeric_hint`, told people to call `(string/->number "1F" 16)`. It took two
/// arguments nowhere: the arity was `exact(1)`, so the hint named a call that raised, and
/// the language could not read a radix at all (found 2026-08-30). The *literals* arrived
/// later (ADR-334): `0xFF` in source is the reader's job; this function reads text a
/// program holds at runtime, and the two never overlap — see the prefix rule below.
///
/// With a radix the parse is **integer-only**, for every radix including 10: a radix
/// describes an integer notation, and `"3.5"` in base 16 is not a number anyone means.
/// A leading `-`/`+` is accepted; a `0x`/`0b`/`0o` *prefix* is not (the digits alone —
/// the prefix is reader syntax, and a radix argument already says the base). Out of `2..=36` the radix
/// is a caller bug, so it raises rather than answering `nil`, which would be
/// indistinguishable from unparseable text.
/// How `string/->number` reads a string — heap-free, allocation-free, and the SINGLE
/// classification both the runtime below and the type checker use.
///
/// The checker specializes `(string/->number "1")` to the literal `1` rather than the
/// declared `number | failure`, which is only honest if it decides parseability exactly the
/// way the runtime does. Two implementations of "is this text a number" would drift, and the
/// drift would be a type that contradicts the value — so there is one, called twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NumericText {
    /// Parses as an `i64` — the checker can name the exact literal.
    Int(i64),
    /// An integer past the `i64` range: a bignum, never a lossy `f64`.
    Big,
    /// Parses only as a float (never reached when a radix is given — a radix describes an
    /// integer notation).
    Float,
    /// `string/->number` answers a `failure` for this text.
    NotANumber,
}

/// Classify `s` the way [`string_to_number`] does. `radix` mirrors the optional second
/// argument; out-of-range radices are the caller's problem (the runtime raises, and the
/// checker simply declines to specialize).
pub(crate) fn classify_numeric_text(s: &str, radix: Option<u32>) -> NumericText {
    if let Some(radix) = radix {
        if !(2..=36).contains(&radix) {
            return NumericText::NotANumber;
        }
        return match i64::from_str_radix(s, radix) {
            Ok(i) => NumericText::Int(i),
            Err(_) => match num_bigint::BigInt::parse_bytes(s.as_bytes(), radix) {
                Some(_) => NumericText::Big,
                None => NumericText::NotANumber,
            },
        };
    }
    if let Ok(i) = s.parse::<i64>() {
        NumericText::Int(i)
    } else if s.parse::<num_bigint::BigInt>().is_ok() {
        NumericText::Big
    } else if s.parse::<f64>().is_ok() {
        NumericText::Float
    } else {
        NumericText::NotANumber
    }
}

pub(super) fn string_to_number(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string/->number", arg(args, 0))?;
    if args.len() >= 2 {
        let radix = expect_int(heap, "string/->number", arg(args, 1))?;
        if !(2..=36).contains(&radix) {
            return Err(LispError::type_err(format!(
                "string/->number: radix must be between 2 and 36, got {radix}"
            )));
        }
        let radix = radix as u32;
        // Same no-demotion reasoning as the decimal path below: past i64 the value is a
        // bignum, never a lossy f64. Classification is shared with the checker.
        return Ok(match classify_numeric_text(&s, Some(radix)) {
            NumericText::Int(i) => Value::int(i),
            NumericText::Big | NumericText::Float => {
                match num_bigint::BigInt::parse_bytes(s.as_bytes(), radix) {
                    Some(n) => heap.alloc_bigint(n),
                    None => heap.alloc_failure(&format!(
                        "string/->number: not a base-{radix} integer: {s:?}"
                    )),
                }
            }
            NumericText::NotANumber => heap.alloc_failure(&format!(
                "string/->number: not a base-{radix} integer: {s:?}"
            )),
        });
    }
    match classify_numeric_text(&s, None) {
        NumericText::Int(i) => return Ok(Value::int(i)),
        NumericText::NotANumber => {
            // Text this cannot interpret is a *known failure*, not an absence: it comes
            // back naming the input, so anything that looks at the value learns why.
            return Ok(heap.alloc_failure(&format!("string/->number: not a number: {s:?}")));
        }
        NumericText::Big | NumericText::Float => {}
    }
    if let Ok(n) = s.parse::<num_bigint::BigInt>() {
        // An integer too big for i64 is a bignum — mirroring the reader's
        // over-range literal path — NOT a lossy f64 (which silently rounded
        // `(str big)` away from round-tripping, kernel audit).
        // Reaching here means the i64 parse failed, so `n` is out of range
        // and `alloc_bigint`'s no-demotion invariant holds.
        Ok(heap.alloc_bigint(n))
    } else if let Ok(f) = s.parse::<f64>() {
        Ok(Value::float(f))
    } else {
        // Text this cannot interpret is a *known failure*, not an absence: it comes
        // back as a falsy `failure` naming the input, so `(or (string/->number s) 0)`
        // still defaults while anything that looks at the value learns why.
        Ok(heap.alloc_failure(&format!("string/->number: not a number: {s:?}")))
    }
}
