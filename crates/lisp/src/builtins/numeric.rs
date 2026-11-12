//! Numeric primitives: the binary arithmetic and comparison kernels behind `+ - * / <`,
//! ratios, `math/floor`, `bit/*` and decimals.
use super::args::{arg, expect_bigint, expect_int, expect_number, expect_string, two};
use super::realize_seqview;
use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};
use std::cmp::Ordering;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::core::value::Tag;
    use crate::types::Sig;
    use crate::types::Ty;
    // numeric primitives — `%add`..`%div` accept and return the wider NUMBER
    // (int + int may overflow into a bignum; a Float argument makes the result a Float).
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
    // passes through to so the VM inlines it as one op, rather than three dispatched
    // calls (`(/ (- a (math/rem a b)) b)`) per step of a tight integer loop.
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
    // `floor` is the single irreducible Float→Int crossing; ceil/round/pow/
    // sqrt are all Brood over it + rem/`/`/`*`/`<` (std/prelude.blsp).
    primitives.def(
        "math/floor",
        Arity::exact(1),
        Sig::new(vec![num], int),
        &["x"],
        "Round x toward negative infinity to an integer. Accepts the whole numeric tower; a ratio floors exactly (not through f64), so it stays correct past 2^53. Toward NEGATIVE infinity, so a negative argument rounds away from zero.\n\n    (math/floor 3.7)    ;=> 3\n    (math/floor -3.2)   ;=> -4",
        floor);
    // bitwise — integer bit-twiddling on the i64 two's-complement representation.
    // Table stakes for hashing, flags, and PRNGs (the std xorshift PRNG is built
    // on these); they were a noted gap (docs/feedback-retro-game-of-life.md).
    primitives.def(
        "bit/and",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        &["a", "b"],
        "Bitwise AND of integers a and b.\n\n    (bit/and 12 10)   ;=> 8",
        bit_and,
    );
    primitives.def(
        "bit/or",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        &["a", "b"],
        "Bitwise (inclusive) OR of integers a and b.\n\n    (bit/or 12 10)   ;=> 14",
        bit_or,
    );
    primitives.def(
        "bit/xor",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        &["a", "b"],
        "Bitwise exclusive-OR of integers a and b.\n\n    (bit/xor 12 10)   ;=> 6",
        bit_xor,
    );
    primitives.def(
        "bit/not",
        Arity::exact(1),
        Sig::new(vec![int], int),
        &["a"],
        "Bitwise complement of integer a (two's-complement, so (bit/not n) = (- (- n) 1)).\n\n    (bit/not 12)   ;=> -13",
        bit_not,
    );
    primitives.def(
        "bit/shift-left",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        &["a", "n"],
        "Shift integer a left by n bits (0 <= n < 64); bits shifted past bit 63 are discarded.\n\n    (bit/shift-left 12 10)   ;=> 12288",
        bit_shift_left,
    );
    primitives.def(
        "bit/shift-right",
        Arity::exact(2),
        Sig::new(vec![int, int], int),
        &["a", "n"],
        "Arithmetic (sign-preserving) right shift of integer a by n bits (0 <= n < 64).\n\n    (bit/shift-right 12 10)   ;=> 0",
        bit_shift_right,
    );
    primitives.def(
        "bit/count",
        Arity::exact(1),
        Sig::new(vec![int], int),
        &["a"],
        "Population count: the number of 1 bits in integer a's two's-complement representation (a negative a counts its sign bits, so (bit/count -1) = 64). For a bignum it is the popcount of the magnitude.\n\n    (bit/count 12)   ;=> 2",
        bit_count);
    primitives.def(
        "bit/positions",
        Arity::exact(1),
        Sig::new(vec![int], vec_ty),
        &["a"],
        "A vector of the 0-based bit indices set in non-negative integer a, ascending (e.g. (bit/positions 6) = [1 2]). O(number of set bits) — for a bignum it scans the magnitude. The inverse of summing (bit/shift-left 1 i); handy for enumerating the set bits of an integer.\n\n    (bit/positions 12)   ;=> [2 3]",
        bit_positions);
    // Bit-level reinterpretation of a binary64 — not expressible over the other
    // primitives (no bitcast, no frexp), and the only way to compare two floats
    // *exactly* (`-0.0` vs `0.0`, NaN payloads). Used by the conformance corpora.
    primitives.def(
        "bit/float->",
        Arity::exact(1),
        Sig::new(vec![num], int),
        &["x"],
        "The IEEE 754 binary64 bit pattern of x, as a non-negative integer (a bignum when the sign bit is set). Reinterpretation, not conversion — the only exact float comparison there is: it separates -0.0 from 0.0 and distinguishes NaN payloads, both of which = collapses. The inverse of bits->float.\n\n    (bit/float-> 12)   ;=> 4622945017495814144",
        float_to_bits);
    primitives.def(
        "bit/->float",
        Arity::exact(1),
        Sig::new(vec![int], float),
        &["n"],
        "The binary64 float whose bit pattern is n (0 <= n < 2^64). The inverse of float->bits.\n\n    (bit/->float 12)   ;=> 6e-323",
        bits_to_float,
    );
    // `decimal/of` is THE way to a decimal (ADR-718): a string ("1.50"), an int (3), a
    // bignum, a ratio (exact when it terminates), or a float (its shortest round-trip form).
    primitives.def(
        "decimal/of",
        Arity::exact(1),
        Sig::new(vec![string.union(num)], decimal_ty),
        &["x"],
        "Construct an exact arbitrary-precision base-10 decimal from x: a string (\"1.50\"), an int (3), a bignum, a ratio (exact when it terminates, `1/2` → `0.5M`; a non-terminating one rounds to the default precision), or a float (converted from its shortest round-trip form, since a float is inexact). For money / Postgres numeric — values a float can't hold exactly. The literal form is a trailing M, e.g. 1.50M.\n\n    (decimal/of \"1.50\")   ;=> 1.50M\n    (decimal/of 1/2)   ;=> 0.5M",
        prim_decimal);
    primitives.def(
        "decimal/->string",
        Arity::exact(1),
        Sig::new(vec![decimal_ty], string),
        &["d"],
        "The canonical decimal string of decimal d (no M suffix).\n\n    (decimal/->string (decimal/of \"1.50\"))   ;=> \"1.50\"",
        prim_decimal_to_string,
    );
    primitives.def(
        "decimal/->float",
        Arity::exact(1),
        Sig::new(vec![decimal_ty], float),
        &["d"],
        "Decimal d as an (inexact) float.\n\n    (decimal/->float (decimal/of \"1.50\"))   ;=> 1.5",
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
        "Parse s strictly as an int (a bignum when out of i64 range), else a float, else a failure value naming the input (truthy, like every failure: test it with failure? — unlike reflect/read-string). The inverse of str. With radix (2-36) the parse is integer-only in that base — how hex/octal/binary is read from DATA (in source, write the literal: 0xFF, 0b1010, 0o17); give the digits alone, no 0x/0b/0o prefix. A radix outside 2-36 raises.\n\n    (string/->number \"42\")   ;=> 42\n    (string/->number \"1F\" 16)   ;=> 31",
        string_to_number);
}

// ---------- numeric ----------

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
/// *native* allocation, which the ADR-043 `BROOD_MEM_LIMIT` cap never sees
/// (`(+ 1/2 (decimal/of "1e-1000000000"))` would climb without end). A million digits is
/// already far past any real decimal; beyond it the operation raises a clean, catchable
/// error.
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
        (Value::Record { .. }, _) | (_, Value::Record { .. }) => {
            num_multi_dispatch(heap, who, args, a, b)
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

/// True when `v` is a record (ADR-542).
fn is_record(_heap: &Heap, v: Value) -> bool {
    matches!(v, Value::Record { .. })
}

/// Dispatch an arithmetic operator to the `Num` MULTIMETHOD (ADR-179) when a record is an
/// operand: `+`/`-`/`*`/`/` → `num/add`/`num/sub`/`num/mul`/`num/div`, each dispatching on
/// BOTH operand identities. Reached ONLY on the cold fallback — the JIT/VM inlines int+int
/// and float+float and never calls `%add` for them, so the numeric hot path is byte-for-byte
/// untouched (a Brood `(record? a)` branch in `+` would cost orders of magnitude). A pair
/// with no method raises the multimethod's loud `no-method` error, so mixed types are
/// explicit, never silently coerced.
///
/// Called by an engine that offered to make the call itself (ADR-505), it hands the
/// multimethod back instead of running it under this builtin's frame, so a `receive` in the
/// method parks; called from Rust (`add_values`), it runs it nested as before.
fn num_multi_dispatch(
    heap: &mut Heap,
    who: &str,
    args: &[Value],
    a: Value,
    b: Value,
) -> LispResult {
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
    if heap.redirect_offered(args) {
        return Err(heap.redirect(callee, &[a, b]));
    }
    crate::eval::compile::apply_engine(heap, callee, &[a, b], genv)
}

/// `<` or `<=` on a record, handed back to the engine that offered to make the call
/// (ADR-505): `shim` is the prelude's `%ord-lt` / `%ord-le`, which calls `compare-to` and reads
/// its sign as [`compare_multi_dispatch`] does. `None` when the call was not offered, an
/// operand is no record, or the prelude has not defined the shim yet — the caller then
/// compares here, nested.
fn ordering_redirect(
    heap: &mut Heap,
    args: &[Value],
    shim: &str,
    a: Value,
    b: Value,
) -> Option<LispError> {
    if !(is_record(heap, a) || is_record(heap, b)) {
        return None;
    }
    let genv = heap.global();
    let callee = heap.env_get(genv, crate::core::value::intern(shim))?;
    if !heap.redirect_offered(args) {
        return None;
    }
    Some(heap.redirect(callee, &[a, b]))
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
    match crate::eval::compile::apply_engine(heap, callee, &[a, b], genv)? {
        Value::Int(n) => Ok(n.cmp(&0)),
        _ => Err(LispError::runtime(format!(
            "{who}: `compare-to` must return an int (-1/0/1)"
        ))),
    }
}

/// `(+ a b)` for the kernel's own use — the same tower dispatch and the same errors
/// (naming `+`) as the primitive, without an argument slice. `core::table::add` is the
/// caller: the linear-map rewrite's fused read-modify-write has to add exactly as the
/// `+` it replaced did.
pub(crate) fn add_values(heap: &mut Heap, a: Value, b: Value) -> LispResult {
    let env = heap.global();
    prim_add(&[a, b], env, heap)
}

/// `(- a b)` for the kernel's own use — `add_values`'s sibling, for `core::table::sub`.
pub(crate) fn sub_values(heap: &mut Heap, a: Value, b: Value) -> LispResult {
    let env = heap.global();
    prim_sub(&[a, b], env, heap)
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
        return num_multi_dispatch(heap, "/", args, a, b);
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

/// The shared `division by zero` error (`/` raises rather than returning IEEE ∞).
fn div_by_zero() -> LispError {
    LispError::runtime("division by zero")
        .with_code(crate::error::error_codes::DIV_BY_ZERO)
        .with_hint("guard the denominator: (when (not (= y 0)) (/ x y))")
}

/// How `a` orders against `b` — the one comparison `%lt`, `%le`, `%max` and `%min` share.
/// `None` only for two numbers with no order (a NaN), which every caller must answer as
/// "not less, not greater": `(<= nan 1)` is false, so `<=` matches `Less | Equal` rather
/// than testing `!= Greater`.
///
/// Two integers compare directly; coercing to f64 first loses precision past 2^53
/// (`(< 9007199254740992 9007199254740993)` would wrongly be false). `value_cmp` handles
/// Int/BigInt exactly and the mixed int/float and BigInt/float cases. A record operand
/// (a map with a truthy `__id__`) routes to the `Ord` multimethod; gating on the Map
/// *tag* keeps the record check off the numeric path.
fn ordering(heap: &mut Heap, who: &str, a: Value, b: Value) -> Result<Option<Ordering>, LispError> {
    Ok(match (a, b) {
        (Value::Int(x), Value::Int(y)) => Some(x.cmp(&y)),
        _ if both_exact(a, b) => Some(heap.value_cmp(a, b)),
        (Value::Record { .. }, _) | (_, Value::Record { .. }) => {
            Some(compare_multi_dispatch(heap, who, a, b)?)
        }
        _ => num_to_f64(heap, who, a)?.partial_cmp(&num_to_f64(heap, who, b)?),
    })
}

pub(super) fn prim_lt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "<")?;
    if let Some(redirect) = ordering_redirect(heap, args, "%ord-lt", a, b) {
        return Err(redirect);
    }
    Ok(Value::boolean(
        ordering(heap, "<", a, b)? == Some(Ordering::Less),
    ))
}

/// `(%le a b)` — `a <= b`. The `<=`/`>=` kernel: a direct primitive so the 2-arg
/// clauses of `<=`/`>=` are pure passthroughs the ADR-069 thin-wrapper elision can
/// reach (the old `(not (%lt …))` bodies were a nested call it couldn't).
pub(super) fn prim_le(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "<=")?;
    if let Some(redirect) = ordering_redirect(heap, args, "%ord-le", a, b) {
        return Err(redirect);
    }
    let le = matches!(
        ordering(heap, "<=", a, b)?,
        Some(Ordering::Less | Ordering::Equal)
    );
    Ok(Value::boolean(le))
}

/// The extreme of `args` (at least one, by arity): each later value replaces the best so
/// far when the best orders `replace_when` against it.
fn extremum(heap: &mut Heap, who: &str, args: &[Value], replace_when: Ordering) -> LispResult {
    let mut best = args[0];
    for &v in &args[1..] {
        if ordering(heap, who, best, v)? == Some(replace_when) {
            best = v;
        }
    }
    Ok(best)
}

pub(super) fn prim_max(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    extremum(heap, "math/max", args, Ordering::Less)
}

pub(super) fn prim_min(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    extremum(heap, "math/min", args, Ordering::Greater)
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
    let (a, b) = two(args, "math/rem")?;
    // i64 fast path. `checked_rem` returns None on `b == 0` (div-by-zero) and
    // on the lone `i64::MIN % -1` overflow — that overflow is mathematically 0,
    // so handle it directly rather than promoting.
    if let (Value::Int(x), Value::Int(y)) = (a, b) {
        return match x.checked_rem(y) {
            Some(r) => Ok(Value::int(r)),
            None if y == 0 => Err(LispError::runtime("%rem: division by zero")
                .with_code(crate::error::error_codes::DIV_BY_ZERO)
                .with_hint("guard the denominator: (when (not (= y 0)) (math/rem x y))")),
            None => Ok(Value::int(0)), // i64::MIN % -1
        };
    }
    let (x, y) = bigint_pair(heap, args, "math/rem")?;
    if num_traits::Zero::is_zero(&y) {
        return Err(LispError::runtime("%rem: division by zero")
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
    let (a, b) = two(args, "math/quot")?;
    if let (Value::Int(x), Value::Int(y)) = (a, b) {
        match x.checked_div(y) {
            Some(q) => return Ok(Value::int(q)),
            None if y == 0 => {
                return Err(LispError::runtime("%quot: division by zero")
                    .with_code(crate::error::error_codes::DIV_BY_ZERO)
                    .with_hint("guard the denominator: (when (not (= y 0)) (math/quot x y))"));
            }
            None => {} // i64::MIN / -1 — promote and fall through
        }
    }
    let (x, y) = bigint_pair(heap, args, "math/quot")?;
    if num_traits::Zero::is_zero(&y) {
        return Err(LispError::runtime("%quot: division by zero")
            .with_code(crate::error::error_codes::DIV_BY_ZERO)
            .with_hint("guard the denominator: (when (not (= y 0)) (math/quot x y))"));
    }
    // `BigInt::/` truncates toward zero, matching i64 `checked_div`.
    Ok(heap.int_from_bigint(x / y))
}

/// Floor toward negative infinity, returning an `Int` — the one Float→Int
/// crossing the language can't bootstrap (no other primitive produces an `Int`
/// from a `Float`). An `Int` passes through; a `Float` is floored. `NaN` and
/// infinities are runtime errors, never a silently saturated `as i64` cast; a finite
/// floor past `i64` promotes to a bignum. `ceil`/`round`/`quot`/
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
            let f = num_to_f64(heap, "math/floor", v)?.floor();
            if !f.is_finite() {
                return Err(LispError::runtime(format!(
                    "math/floor: argument {} has no integer floor",
                    f
                ))
                .with_code(crate::error::error_codes::INT_OVERFLOW));
            }
            // `i64::MIN as f64` rounds *down* to a value still in range; the
            // upper bound `i64::MAX as f64` rounds *up* past `i64::MAX`, so
            // the open upper comparison is the right one.
            if f < i64::MIN as f64 || f >= i64::MAX as f64 + 1.0 {
                // Finite but past i64: PROMOTE, as `+`/`*` do on overflow. Every f64
                // of that magnitude is already an integer, so the conversion is exact
                // — `(math/floor 1e300)` is the 10^300-ish integer the double holds,
                // not an error: a language that promotes on overflow must not have a
                // cliff at 2^63 in its one float->int crossing.
                use num_traits::FromPrimitive;
                let big = num_bigint::BigInt::from_f64(f).expect("a finite f64 converts");
                return Ok(heap.alloc_bigint(big));
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
            "{}: shift amount {} too large (max {})",
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
    if let Value::Int(x) = a
        && amount < 64
        && let Some(r) = x.checked_shl(amount as u32)
    {
        // checked_shl only guards the *shift amount*, not value overflow;
        // verify the shift is lossless before keeping the i64 result.
        if (r >> amount) == x {
            return Ok(Value::int(r));
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

/// `(decimal/of x)` — the one way to an exact base-10 `Decimal` (ADR-718). A string parses
/// exactly; an int or `BigInt` is exact; a ratio is exact when it terminates; a float is
/// converted from its *shortest round-trip* decimal text (an f64 is inexact, so
/// `(decimal/of 0.1)` is the decimal `0.1`, the value the literal `0.1` denotes, not the
/// full binary expansion — which is what the removed `decimal/number->` gave).
pub(super) fn prim_decimal(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use std::str::FromStr;
    let v = arg(args, 0);
    let d = match v {
        // Already a decimal — return it unchanged.
        Value::Decimal(_) => return Ok(v),
        Value::Int(n) => bigdecimal::BigDecimal::from(n),
        Value::BigInt(id) => bigdecimal::BigDecimal::from(heap.bigint(id).clone()),
        // Exact when the ratio terminates (`1/2` → `0.5M`); `1/3` rounds to bigdecimal's
        // default precision.
        Value::Ratio(id) => {
            let ratio = heap.ratio(id).clone();
            bigdecimal::BigDecimal::from(ratio.numer().clone())
                / bigdecimal::BigDecimal::from(ratio.denom().clone())
        }
        Value::Float(f) => {
            if !f.is_finite() {
                return Err(LispError::runtime(format!(
                    "decimal/of: cannot represent non-finite float {f}"
                )));
            }
            // Shortest round-trip text (what the printer would show), parsed exactly.
            bigdecimal::BigDecimal::from_str(&format_decimal_from_float(f))
                .map_err(|_| LispError::runtime(format!("decimal/of: cannot convert float {f}")))?
        }
        Value::Str(id) => {
            let s = heap.string(id).trim().to_string();
            bigdecimal::BigDecimal::from_str(&s).map_err(|_| {
                LispError::runtime(format!("decimal/of: malformed decimal string {s:?}"))
            })?
        }
        other => {
            return Err(LispError::wrong_type(
                heap,
                "decimal/of",
                "string or number",
                other,
            ));
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
            ));
        }
    };
    Ok(heap.alloc_string_owned(s))
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
    // A bignum past f64's range reads as `inf` through `num_to_f64`, and `inf.sqrt()`
    // is `inf` — a wrong answer for an input whose root is perfectly representable
    // (`(math/sqrt (math/pow 10 400))` is 1e200). Take an even power of two out
    // first: sqrt(m * 2^2k) = sqrt(m) * 2^k, with `m` sized to fit a double.
    if let Value::BigInt(id) = arg(args, 0) {
        use num_traits::{Signed, ToPrimitive};
        let n = heap.bigint(id).clone();
        if n.is_negative() {
            return Err(LispError::runtime(format!(
                "%f64-sqrt: argument {} must be non-negative",
                n
            )));
        }
        let bits = n.bits();
        if bits > 1000 {
            let shift = ((bits - 1000) & !1) as usize;
            let m = (&n >> shift).to_f64().unwrap_or(f64::INFINITY);
            return Ok(Value::float(m.sqrt() * 2f64.powi((shift / 2) as i32)));
        }
    }
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
/// a float, else a **failure** value naming the input (truthy like every failure, so `(or … d)`
/// does NOT default it — test `failure?`; it says *why* where `nil` could not). The inverse of `str`. A robust parse can't be
/// expressed over `reflect/read-string` (which would read `"3abc"` as `3` and stop), so
/// the strict parse is a primitive. Surrounding whitespace is not accepted —
/// `trim` first if the input may carry any.
///
/// **`radix` is how hex, octal or binary is read from DATA.** ADR-169 reserved
/// `0x1F`/`0b1010`/`0o17` as syntax rather than names *on the grounds that this function
/// covers the need at runtime*, and the reader's own hint (`syntax/atom.rs`'s
/// `reserved_numeric_hint`) names `(string/->number "1F" 16)`. The *literals* are the
/// reader's job (ADR-334); this function reads text a program holds at runtime, and the
/// two never overlap — see the prefix rule below.
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
    // Brood has no digit separators (the reader refuses `1_000` as reserved syntax), but
    // `num_bigint`'s parsers accept `_` between digits where `i64`'s and `f64`'s do not.
    // Left to them, `"1_000"` classified as an over-range `Big` and the runtime built a
    // bignum holding 1000: a debug-build panic, and in release an `Int`/`BigInt` pair that
    // compared unequal at the same value. Refuse it once here, for runtime and checker both.
    if s.contains('_') {
        return NumericText::NotANumber;
    }
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
                    Some(n) => heap.int_from_bigint(n),
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
        // Reaching here means the i64 parse failed, so `n` should be out of range;
        // `int_from_bigint` normalizes anyway, so a parser that accepts more than
        // `i64`'s (it once took `_` separators) cannot build an in-range bignum.
        Ok(heap.int_from_bigint(n))
    } else if let Ok(f) = s.parse::<f64>() {
        Ok(Value::float(f))
    } else {
        // Text this cannot interpret is a *known failure*, not an absence: it comes
        // back as a `failure` naming the input — truthy, so `(or (string/->number s) 0)`
        // hands the failure on rather than defaulting it, and whoever looks learns why.
        Ok(heap.alloc_failure(&format!("string/->number: not a number: {s:?}")))
    }
}
