use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::core::keywords as kw;
use super::realize_seqview;
#[allow(unused_macros)]
macro_rules! expect {
    ($heap:expr, $who:expr, $v:expr, $expected:literal, $($pat:pat => $extract:expr),+ $(,)?) => {
        match $v {
            $($pat => Ok($extract),)+
            __other => Err(LispError::wrong_type($heap, $who, $expected, __other)),
        }
    };
}

pub(super) fn arg(args: &[Value], i: usize) -> Value {
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

/// Require a value of a particular shape, or raise a self-identifying type
/// error attributed to `who` (the primitive that needed it). One macro behind
/// every `expect_*` helper below — the alternative was six hand-written
/// `match v { Value::X(id) => Ok(id), _ => Err(wrong_type(…, "kind", v)) }`
/// copies that drifted on the error helper used (`expect_node_name` chose
/// `type_err` over `wrong_type` and lost the offending value from its
/// message). The macro lifts that one rule into one place; the human-readable
/// `$expected` string is what the error message will say.
macro_rules! expect {
    ($heap:expr, $who:expr, $v:expr, $expected:literal, $($pat:pat => $extract:expr),+ $(,)?) => {
        match $v {
            $($pat => Ok($extract),)+
            __other => Err(LispError::wrong_type($heap, $who, $expected, __other)),
        }
    };
}

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
pub(super) fn expect_bigint(heap: &Heap, who: &str, v: Value) -> Result<num_bigint::BigInt, LispError> {
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

/// Coerce an integer-or-float `Value` to `f64` for the float arithmetic path —
/// like [`expect_number`] but a `BigInt` also coerces (via its `to_f64`), so a
/// mixed `(+ 2^200 1.5)` works rather than rejecting the bignum.
pub(super) fn num_to_f64(heap: &Heap, who: &str, v: Value) -> Result<f64, LispError> {
    use num_traits::ToPrimitive;
    match v {
        Value::BigInt(id) => Ok(heap.bigint(id).to_f64().unwrap_or(f64::INFINITY)),
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
        // A float operand anywhere: the float path (a BigInt coerces via `f64`).
        _ => Ok(Value::Float(float_op(
            num_to_f64(heap, who, a)?,
            num_to_f64(heap, who, b)?,
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
    )
}

pub(super) fn prim_div(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "/")?;
    let bf = num_to_f64(heap, "/", b)?;
    if bf == 0.0 {
        return Err(LispError::runtime("division by zero")
            .with_code(crate::error::error_codes::DIV_BY_ZERO)
            .with_hint("guard the denominator: (when (not= y 0) (/ x y))"));
    }
    match (a, b) {
        // Exact integer quotient when it divides evenly; otherwise a float.
        // `checked_*` guards the one overflowing case (`i64::MIN / -1`), which
        // then falls through to the float path instead of panicking.
        (Value::Int(x), Value::Int(y)) => match (x.checked_rem(y), x.checked_div(y)) {
            (Some(0), Some(q)) => Ok(Value::int(q)),
            _ => Ok(Value::Float(x as f64 / y as f64)),
        },
        // Both integers, at least one a BigInt: exact quotient when it divides
        // evenly (demoted — `(/ 2^200 2^100)` is the exact `2^100`); otherwise a
        // float. Division by zero was already caught via `bf == 0.0`.
        _ if is_integer(a) && is_integer(b) => {
            use num_integer::Integer;
            let x = heap.as_bigint(a).expect("integer");
            let y = heap.as_bigint(b).expect("integer");
            let (q, r) = x.div_rem(&y);
            if num_traits::Zero::is_zero(&r) {
                Ok(heap.int_from_bigint(q))
            } else {
                Ok(Value::Float(num_to_f64(heap, "/", a)? / bf))
            }
        }
        _ => Ok(Value::Float(num_to_f64(heap, "/", a)? / bf)),
    }
}

pub(super) fn prim_lt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "<")?;
    // Compare two integers directly; coercing to f64 first loses precision past
    // 2^53 (e.g. `(< 9007199254740992 9007199254740993)` would wrongly be false).
    // `value_cmp` already handles Int/BigInt exactly and the mixed int/float and
    // BigInt/float cases.
    let lt = match (a, b) {
        (Value::Int(x), Value::Int(y)) => x < y,
        _ if is_integer(a) && is_integer(b) => heap.value_cmp(a, b) == std::cmp::Ordering::Less,
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
        _ if is_integer(a) && is_integer(b) => heap.value_cmp(a, b) != std::cmp::Ordering::Greater,
        _ => num_to_f64(heap, "<=", a)? <= num_to_f64(heap, "<=", b)?,
    };
    Ok(Value::boolean(le))
}

pub(super) fn prim_max(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut best = args[0]; // Arity::at_least(1) ensures non-empty
    for &v in &args[1..] {
        let replace = match (best, v) {
            (Value::Int(a), Value::Int(b)) => b > a,
            _ if is_integer(best) && is_integer(v) => {
                heap.value_cmp(best, v) == std::cmp::Ordering::Less
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
            _ if is_integer(best) && is_integer(v) => {
                heap.value_cmp(best, v) == std::cmp::Ordering::Greater
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
                .with_hint("guard the denominator: (when (not= y 0) (rem x y))")),
            None => Ok(Value::int(0)), // i64::MIN % -1
        };
    }
    let (x, y) = bigint_pair(heap, args, "rem")?;
    if num_traits::Zero::is_zero(&y) {
        return Err(LispError::runtime("rem: division by zero")
            .with_code(crate::error::error_codes::DIV_BY_ZERO)
            .with_hint("guard the denominator: (when (not= y 0) (rem x y))"));
    }
    // `BigInt::%` truncates toward zero (matches i64 `%`), so the remainder has
    // the dividend's sign — the non-Euclidean `rem` the prelude `mod` builds on.
    Ok(heap.int_from_bigint(x % y))
}

/// `(%quot a b)` — truncating integer division toward zero, the kernel `quot`
/// passes through to. `checked_div` truncates toward zero (matching the old
/// `(/ (- a (rem a b)) b)` integer result) and guards both `b == 0` and the lone
/// `i64::MIN / -1` overflow; that overflow promotes to BigInt.
pub(super) fn prim_quot(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "quot")?;
    if let (Value::Int(x), Value::Int(y)) = (a, b) {
        match x.checked_div(y) {
            Some(q) => return Ok(Value::int(q)),
            None if y == 0 => {
                return Err(LispError::runtime("quot: division by zero")
                    .with_code(crate::error::error_codes::DIV_BY_ZERO)
                    .with_hint("guard the denominator: (when (not= y 0) (quot x y))"))
            }
            None => {} // i64::MIN / -1 — promote and fall through
        }
    }
    let (x, y) = bigint_pair(heap, args, "quot")?;
    if num_traits::Zero::is_zero(&y) {
        return Err(LispError::runtime("quot: division by zero")
            .with_code(crate::error::error_codes::DIV_BY_ZERO)
            .with_hint("guard the denominator: (when (not= y 0) (quot x y))"));
    }
    // `BigInt::/` truncates toward zero, matching i64 `checked_div`.
    Ok(heap.int_from_bigint(x / y))
}

/// Floor toward negative infinity, returning an `Int` — the one Float→Int
/// crossing the language can't bootstrap (no other primitive produces an `Int`
/// from a `Float`). An `Int` passes through; a `Float` is floored. `NaN` and
/// values whose floor doesn't fit in `i64` are runtime errors — pre-fix the
/// `as i64` cast silently saturated, so `(floor (* 1e308 1e308))` returned
/// `i64::MAX` and `(floor (/ 0.0 0.0))` returned `0`. `ceil`/`round`/`quot`/
/// `pow`/`sqrt` are all Brood over this + `rem`/`/`/`*`/`<` (std/prelude.blsp).
pub(super) fn floor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Int(n) => Ok(Value::int(n)),
        // A bignum is already an integer — it is its own floor.
        v @ Value::BigInt(_) => Ok(v),
        v => {
            let f = expect_number(heap, "floor", v)?.floor();
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
    let (a, b) = bigint_pair(heap, args, "bit-and")?;
    Ok(heap.int_from_bigint(a & b))
}

pub(super) fn bit_or(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if let (Value::Int(a), Value::Int(b)) = (arg(args, 0), arg(args, 1)) {
        return Ok(Value::int(a | b));
    }
    let (a, b) = bigint_pair(heap, args, "bit-or")?;
    Ok(heap.int_from_bigint(a | b))
}

pub(super) fn bit_xor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if let (Value::Int(a), Value::Int(b)) = (arg(args, 0), arg(args, 1)) {
        return Ok(Value::int(a ^ b));
    }
    let (a, b) = bigint_pair(heap, args, "bit-xor")?;
    Ok(heap.int_from_bigint(a ^ b))
}

pub(super) fn bit_not(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Int(n) => Ok(Value::int(!n)),
        Value::BigInt(id) => {
            let n = !heap.bigint(id).clone();
            Ok(heap.int_from_bigint(n))
        }
        v => Err(LispError::wrong_type(heap, "bit-not", "int", v)),
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
        v => Err(LispError::wrong_type(heap, "bit-count", "int", v)),
    }
}

pub(super) fn bit_positions(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // The 0-based indices of the set bits, ascending. O(popcount): pull the
    // lowest set bit, record it, clear it, repeat — so enumerating a sparse
    // bitset costs the number of members, not the bit width (the bitboard
    // renderer leans on this to stay O(live) instead of O(area)).
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
        v => return Err(LispError::wrong_type(heap, "bit-positions", "int", v)),
    }
    Ok(heap.alloc_vector(out))
}

// ===== bitset: refc-shared fixed-size bit array (POC) ============================
// A bitset is read WITHOUT copying its bytes: a shared one hands back its `Arc`
// (a refcount bump), so the bitwise ops touch the blob in place and only the
// RESULT is allocated. (An inline string — only happens off the bitset path —
// falls back to a one-time copy.)
// A bitset is now always an `Arc<SharedBlob>` (KI-4: its own `Value::Bitset` kind, never a
// UTF-8 `Str`), so this is just the shared bytes. Kept as a thin wrapper so the bitset ops
// read through one `.bytes()` accessor.
pub(super) struct BsData(std::sync::Arc<crate::core::blob::SharedBlob>);
impl BsData {
    fn bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

pub(super) fn bs_arc(heap: &Heap, v: Value, who: &str) -> Result<BsData, LispError> {
    match v {
        // A bitset is its own `Value::Bitset` kind (KI-4) — raw bytes via an
        // `Arc<SharedBlob>`, never a UTF-8 `Value::Str`. Read the Arc byte-clean.
        Value::Bitset(id) => Ok(BsData(std::sync::Arc::clone(heap.bitset(id)))),
        _ => Err(LispError::wrong_type(heap, who, "bitset", v)),
    }
}

// Allocate a bitset from raw bytes — a distinct `Value::Bitset` (KI-4), ALWAYS shared so
// send/table ship it by reference. Byte-clean: the bytes are arbitrary, never UTF-8.
pub(super) fn bs_alloc(heap: &mut Heap, bytes: &[u8]) -> Value {
    heap.alloc_bitset(crate::core::blob::SharedBlob::new(bytes))
}

pub(super) fn bs_nbytes(nbits: i64) -> usize {
    ((nbits.max(0) as usize) + 7) / 8
}

pub(super) fn bs_make(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "bitset", arg(args, 0))?;
    Ok(bs_alloc(heap, &vec![0u8; bs_nbytes(n)]))
}

pub(super) fn bs_ones(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let n = expect_int(heap, "bitset-ones", arg(args, 0))?.max(0) as usize;
    let len = (n + 7) / 8;
    let mut out = vec![0xffu8; len];
    if len > 0 && n % 8 != 0 {
        out[len - 1] = (1u8 << (n % 8)) - 1;
    }
    Ok(bs_alloc(heap, &out))
}

pub(super) fn bs_binop(heap: &mut Heap, args: &[Value], who: &str, f: fn(u8, u8) -> u8) -> LispResult {
    let a = bs_arc(heap, arg(args, 0), who)?;
    let b = bs_arc(heap, arg(args, 1), who)?;
    let (ab, bb) = (a.bytes(), b.bytes());
    let out: Vec<u8> = (0..ab.len()).map(|i| f(ab[i], *bb.get(i).unwrap_or(&0))).collect();
    Ok(bs_alloc(heap, &out))
}

// Fused bit-plane full-adder: sum a vector of bitsets bit-by-bit into the low THREE
// planes [s0 s1 s2] of the per-bit count, in ONE native pass (no per-op allocation —
// the ~40-op interpreted reduce this replaces alloc'd a fresh bitset per step). The
// adder is bitwise-parallel: each byte carries 8 independent bit-columns; the carry
// flows between PLANES (s0→s1→s2), not between bits. General over any field count.
pub(super) fn bs_planes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let items: Vec<Value> = match arg(args, 0) {
        Value::Vector(id) => heap.vector(id).to_vec(),
        v => return Err(LispError::wrong_type(heap, "bitset-planes", "vector", v)),
    };
    let datas: Vec<BsData> = items
        .iter()
        .map(|x| bs_arc(heap, *x, "bitset-planes"))
        .collect::<Result<_, _>>()?;
    let len = datas.first().map_or(0, |d| d.bytes().len());
    let (mut s0, mut s1, mut s2) = (vec![0u8; len], vec![0u8; len], vec![0u8; len]);
    for d in &datas {
        let m = d.bytes();
        for j in 0..len {
            let mj = *m.get(j).unwrap_or(&0);
            let c = s0[j] & mj;
            s0[j] ^= mj;
            let c2 = s1[j] & c;
            s1[j] ^= c;
            s2[j] ^= c2;
        }
    }
    let r0 = bs_alloc(heap, &s0);
    let r1 = bs_alloc(heap, &s1);
    let r2 = bs_alloc(heap, &s2);
    Ok(heap.alloc_vector(vec![r0, r1, r2]))
}

pub(super) fn bs_and(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    bs_binop(heap, args, "bitset-and", |x, y| x & y)
}
pub(super) fn bs_or(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    bs_binop(heap, args, "bitset-or", |x, y| x | y)
}
pub(super) fn bs_xor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    bs_binop(heap, args, "bitset-xor", |x, y| x ^ y)
}

pub(super) fn bs_shl(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let srcd = bs_arc(heap, arg(args, 0), "bitset-shl")?;
    let src = srcd.bytes();
    let n = expect_int(heap, "bitset-shl", arg(args, 1))?.max(0) as usize;
    let (len, bo, bs) = (src.len(), n / 8, n % 8);
    let mut out = vec![0u8; len];
    for j in bo..len {
        let lo = src[j - bo] as u16;
        let mut v = lo << bs;
        if bs != 0 && j - bo >= 1 {
            v |= (src[j - bo - 1] as u16) >> (8 - bs);
        }
        out[j] = (v & 0xff) as u8;
    }
    Ok(bs_alloc(heap, &out))
}

pub(super) fn bs_shr(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let srcd = bs_arc(heap, arg(args, 0), "bitset-shr")?;
    let src = srcd.bytes();
    let n = expect_int(heap, "bitset-shr", arg(args, 1))?.max(0) as usize;
    let (len, bo, bs) = (src.len(), n / 8, n % 8);
    let mut out = vec![0u8; len];
    for j in 0..len {
        let hi = j + bo;
        if hi >= len {
            break;
        }
        let mut v = (src[hi] as u16) >> bs;
        if bs != 0 && hi + 1 < len {
            v |= (src[hi + 1] as u16) << (8 - bs);
        }
        out[j] = (v & 0xff) as u8;
    }
    Ok(bs_alloc(heap, &out))
}

pub(super) fn bs_set(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut src = bs_arc(heap, arg(args, 0), "bitset-set")?.bytes().to_vec();
    let i = expect_int(heap, "bitset-set", arg(args, 1))?.max(0) as usize;
    let (byte, bit) = (i / 8, i % 8);
    if byte < src.len() {
        src[byte] |= 1 << bit;
    }
    Ok(bs_alloc(heap, &src))
}

pub(super) fn bs_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let srcd = bs_arc(heap, arg(args, 0), "bitset-count")?;
    let n: u32 = srcd.bytes().iter().map(|b| b.count_ones()).sum();
    Ok(Value::int(n as i64))
}

pub(super) fn bs_positions(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let srcd = bs_arc(heap, arg(args, 0), "bitset-positions")?;
    let src = srcd.bytes();
    let mut out: Vec<Value> = Vec::new();
    for (bi, &byte) in src.iter().enumerate() {
        let mut b = byte;
        while b != 0 {
            out.push(Value::int((bi * 8 + b.trailing_zeros() as usize) as i64));
            b &= b - 1;
        }
    }
    Ok(heap.alloc_vector(out))
}

// Pure byte-buffer bit ops (LSB-first, fixed length) used by the fused step below.
pub(super) fn b_shl(src: &[u8], n: usize) -> Vec<u8> {
    let (len, bo, bs) = (src.len(), n / 8, n % 8);
    let mut out = vec![0u8; len];
    for j in bo..len {
        let mut v = (src[j - bo] as u16) << bs;
        if bs != 0 && j - bo >= 1 {
            v |= (src[j - bo - 1] as u16) >> (8 - bs);
        }
        out[j] = (v & 0xff) as u8;
    }
    out
}
pub(super) fn b_shr(src: &[u8], n: usize) -> Vec<u8> {
    let (len, bo, bs) = (src.len(), n / 8, n % 8);
    let mut out = vec![0u8; len];
    for j in 0..len {
        let hi = j + bo;
        if hi >= len {
            break;
        }
        let mut v = (src[hi] as u16) >> bs;
        if bs != 0 && hi + 1 < len {
            v |= (src[hi + 1] as u16) << (8 - bs);
        }
        out[j] = (v & 0xff) as u8;
    }
    out
}
pub(super) fn b_and(a: &[u8], b: &[u8]) -> Vec<u8> {
    (0..a.len()).map(|i| a[i] & b.get(i).copied().unwrap_or(0)).collect()
}
pub(super) fn b_or(a: &[u8], b: &[u8]) -> Vec<u8> {
    (0..a.len()).map(|i| a[i] | b.get(i).copied().unwrap_or(0)).collect()
}
pub(super) fn b_xor(a: &[u8], b: &[u8]) -> Vec<u8> {
    (0..a.len()).map(|i| a[i] ^ b.get(i).copied().unwrap_or(0)).collect()
}

// The whole Moore-8 torus neighbour sum in ONE native pass: builds the eight
// torus-shifted neighbour fields (west/east with column wrap, then each lifted a
// row up/down with row wrap) and full-adders them into the low 3 count planes
// [s0 s1 s2]. A general life-like-CA primitive; the survival RULE stays in Brood.
// Args: board bits, the precomputed col0/high/mask/board-mask bitsets, w, h.
pub(super) fn bs_neighbour_sum(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "bitset-neighbour-sum";
    let b = bs_arc(heap, arg(args, 0), who)?;
    let col0 = bs_arc(heap, arg(args, 1), who)?;
    let high = bs_arc(heap, arg(args, 2), who)?;
    let mask = bs_arc(heap, arg(args, 3), who)?;
    let board = bs_arc(heap, arg(args, 4), who)?;
    let w = expect_int(heap, who, arg(args, 5))?.max(1) as usize;
    let h = expect_int(heap, who, arg(args, 6))?.max(1) as usize;
    let (bb, c0, hi, mk, bd) = (b.bytes(), col0.bytes(), high.bytes(), mask.bytes(), board.bytes());
    let (wm1, hm1w) = (w - 1, (h - 1) * w);

    // west / east fields (torus-wrapped within each row)
    let l = b_or(&b_and(&b_shl(bb, 1), &b_xor(c0, bd)), &b_shr(&b_and(bb, hi), wm1));
    let r = b_or(&b_and(&b_shr(bb, 1), &b_xor(hi, bd)), &b_shl(&b_and(bb, c0), wm1));
    let up = |f: &[u8]| b_or(&b_and(&b_shl(f, w), bd), &b_shr(f, hm1w));
    let dn = |f: &[u8]| b_or(&b_shr(f, w), &b_shl(&b_and(f, mk), hm1w));
    let ns: [Vec<u8>; 8] = [up(&l), up(bb), up(&r), l.clone(), r.clone(), dn(&l), dn(bb), dn(&r)];

    let len = bb.len();
    let (mut s0, mut s1, mut s2) = (vec![0u8; len], vec![0u8; len], vec![0u8; len]);
    for m in &ns {
        for j in 0..len {
            let mj = m[j];
            let c = s0[j] & mj;
            s0[j] ^= mj;
            let c2 = s1[j] & c;
            s1[j] ^= c;
            s2[j] ^= c2;
        }
    }
    let r0 = bs_alloc(heap, &s0);
    let r1 = bs_alloc(heap, &s1);
    let r2 = bs_alloc(heap, &s2);
    Ok(heap.alloc_vector(vec![r0, r1, r2]))
}

// A whole Conway step in ONE native op: the Moore-8 torus neighbour sum (above)
// plus the B3/S23 survival rule `s1 & ~s2 & (s0 | cur)`, returning the next board
// bitset directly — no intermediate Brood allocation. (Bakes the Life rule, unlike
// the general `bitset-neighbour-sum`.) Args as `bitset-neighbour-sum`.
pub(super) fn bs_life_step(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "bitset-life-step";
    let b = bs_arc(heap, arg(args, 0), who)?;
    let col0 = bs_arc(heap, arg(args, 1), who)?;
    let high = bs_arc(heap, arg(args, 2), who)?;
    let mask = bs_arc(heap, arg(args, 3), who)?;
    let board = bs_arc(heap, arg(args, 4), who)?;
    let w = expect_int(heap, who, arg(args, 5))?.max(1) as usize;
    let h = expect_int(heap, who, arg(args, 6))?.max(1) as usize;
    let (bb, c0, hi, mk, bd) = (b.bytes(), col0.bytes(), high.bytes(), mask.bytes(), board.bytes());
    let (wm1, hm1w) = (w - 1, (h - 1) * w);

    let l = b_or(&b_and(&b_shl(bb, 1), &b_xor(c0, bd)), &b_shr(&b_and(bb, hi), wm1));
    let r = b_or(&b_and(&b_shr(bb, 1), &b_xor(hi, bd)), &b_shl(&b_and(bb, c0), wm1));
    let up = |f: &[u8]| b_or(&b_and(&b_shl(f, w), bd), &b_shr(f, hm1w));
    let dn = |f: &[u8]| b_or(&b_shr(f, w), &b_shl(&b_and(f, mk), hm1w));
    let ns: [Vec<u8>; 8] = [up(&l), up(bb), up(&r), l.clone(), r.clone(), dn(&l), dn(bb), dn(&r)];

    let len = bb.len();
    let mut next = vec![0u8; len];
    for j in 0..len {
        let (mut s0, mut s1, mut s2) = (0u8, 0u8, 0u8);
        for m in &ns {
            let mj = m[j];
            let c = s0 & mj;
            s0 ^= mj;
            let c2 = s1 & c;
            s1 ^= c;
            s2 ^= c2;
        }
        // survive iff (2 neighbours and alive) or 3 neighbours: s1 & ~s2 & (s0 | cur)
        next[j] = s1 & (s2 ^ bd[j]) & (s0 | bb[j]);
    }
    Ok(bs_alloc(heap, &next))
}

/// Validate a shift amount: non-negative (a negative shift is an error) and not
/// absurdly large (cap well above any realistic bit width so a typo'd
/// `(bit-shift-left 1 1e9)` can't try to allocate gigabytes). Returns the amount
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
    let n = expect_int(heap, "bit-shift-left", arg(args, 1))?;
    let amount = shift_amount(n, "bit-shift-left")?;
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
    let x = expect_bigint(heap, "bit-shift-left", a)?;
    Ok(heap.int_from_bigint(x << amount))
}

pub(super) fn bit_shift_right(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let a = arg(args, 0);
    let n = expect_int(heap, "bit-shift-right", arg(args, 1))?;
    let amount = shift_amount(n, "bit-shift-right")?;
    // Arithmetic (sign-preserving) right shift, matching the signed model.
    if let Value::Int(x) = a {
        // A right shift ≥ 64 collapses to the sign bit (0 or -1).
        let r = if amount >= 64 { x >> 63 } else { x >> amount };
        return Ok(Value::int(r));
    }
    let x = expect_bigint(heap, "bit-shift-right", a)?;
    Ok(heap.int_from_bigint(x >> amount))
}


