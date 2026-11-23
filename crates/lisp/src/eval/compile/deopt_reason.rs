//! Why native code deopted: one id per guard site in the JIT lowering.
//!
//! Every guard that branches to an arm's deopt block passes a reason code, which
//! `brood_rt_note_deopt` parks on the heap. The code is a [`DeoptReason`] in the low byte,
//! with an optional payload above it (the observed tag for the int guards, the parameter
//! slot for the entry guard). Deopt feedback decodes it to decide whether to re-tier an arm,
//! and `BROOD_DEOPT_TRACE=1` / `BROOD_JIT_BAIL_TRACE=1` print it by name.
//!
//! Each site has its own variant, so a persistent deopt names the exact guard that fires.
//! `every_reason_has_a_unique_code_and_name` keeps that true.

/// One guard site in the lowering. The discriminant is the low byte of the reason code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub(crate) enum DeoptReason {
    /// A `Done` edge whose operand stack is not exactly one value — unreachable.
    UnreachableDoneEdge = 1,
    /// A block reached with an operand-stack height no `Done` can have — dead code.
    DeadJoinBlock = 2,
    /// A branch on an untagged int whose bool-ness was lost at a merge.
    UntaggedIntBranch = 3,
    /// `first`/`rest` callback declined the operand.
    PairAccessorDeclined = 4,
    /// `empty?` callback declined the operand.
    IsEmptyDeclined = 5,
    /// `sqrt` of an unboxed float that is not positive.
    SqrtFloatNotPositive = 6,
    /// `sqrt` of an unboxed int that is not positive.
    SqrtIntNotPositive = 7,
    /// `sqrt` of a boxed float that is not positive.
    SqrtBoxedFloatNotPositive = 8,
    /// `sqrt` of a boxed value that is neither float nor int.
    SqrtOperandNotNumber = 9,
    /// `sqrt` of a boxed int that is not positive.
    SqrtBoxedIntNotPositive = 10,
    /// `table-put` on a hoisted table declined (its callback fallback).
    HoistedTablePutDeclined = 11,
    /// `table-put` callback declined.
    TablePutDeclined = 12,
    /// Inline element read of a hoisted vector, index out of range.
    HoistedVectorIndexOutOfRange = 13,
    /// Inline element read of a hoisted slot vector, index out of range.
    HoistedSlotVectorIndexOutOfRange = 14,
    /// Checked int `+`/`-`/`*` overflowed (the VM promotes to a bignum).
    IntArithmeticOverflow = 15,
    /// Int `rem`/`quot`/`%div` by zero.
    IntDivideByZero = 16,
    /// Int `rem`/`quot`/`%div` of `i64::MIN` by `-1`.
    IntDivideOverflow = 17,
    /// Int `%div` with a nonzero remainder (the VM answers an exact `Ratio`).
    IntDivideInexact = 18,
    /// Float `/` by zero (the VM raises).
    FloatDivideByZero = 19,
    /// A frame slot read as an int held another type; payload = observed tag.
    SlotIntGuard = 20,
    /// A boxed value read as an int held another type; payload = observed tag.
    HandleIntGuard = 21,
    /// A hoisted global vector/table used as an int.
    HoistedValueNotInt = 22,
    /// An unboxed float where an int is required.
    FloatWhereIntRequired = 23,
    /// A frame slot read as a float held neither float nor (promotable) int.
    SlotFloatGuard = 24,
    /// A boxed value read as a float held neither float nor (promotable) int.
    HandleFloatGuard = 25,
    /// An operand that can never be a float, read as one.
    NeverFloatOperand = 26,
    /// A boolean where a number is required (the VM raises the type error).
    BoolWhereNumberRequired = 27,
    /// Table read callback declined.
    TableReadDeclined = 28,
    /// `=` over a seq-view operand, which only the VM's `%eq` can realise.
    EqualSeqViewOperand = 29,
    /// Inline vector read: the operand is not a vector.
    InlineVectorRefNotVector = 30,
    /// Inline vector read of an inline-stored vector, index out of range.
    InlineVectorRefOutOfRange = 32,
    /// Inline vector read: the element store is neither inline nor spilled.
    InlineVectorRefUnexpectedStore = 33,
    /// Inline vector read of a spilled vector, index out of range.
    InlineVectorRefSpilledOutOfRange = 34,
    /// Inline vector read: the callback fallback declined.
    InlineVectorRefDeclined = 35,
    /// A self-loop back edge saw the global epoch move (a `def` since entry).
    BackEdgeEpochChanged = 36,
    /// A self-loop back edge found a limit waiting to be raised, which only the VM can do.
    BackEdgeLimitPending = 37,
    /// `(get m k default)` callback declined.
    MapGet3Declined = 38,
    /// `(assoc m k v)` callback declined.
    MapAssocDeclined = 39,
    /// `%vector-length` of a non-vector.
    VectorLengthNotVector = 40,
    /// Float pair, both operands provably never floats.
    FloatPairNeverFloat = 41,
    /// Float pair, the right operand beside a float is neither float nor int.
    FloatPairRightBesideFloat = 42,
    /// Float pair, the left operand beside a float is neither float nor int.
    FloatPairLeftBesideFloat = 43,
    /// Float pair, both tagged: the second operand beside a float is neither float nor int.
    FloatPairSecondBesideFloat = 44,
    /// Float pair, both tagged: the first operand is neither float nor int.
    FloatPairFirstNotNumber = 45,
    /// Float pair, both tagged: the first is an int and the second is not a float.
    FloatPairSecondBesideInt = 46,
    /// Mixed comparison: the first operand is neither float nor promotable int.
    CompareFirstOperand = 47,
    /// Mixed comparison: the second operand is neither float nor promotable int.
    CompareSecondOperand = 48,
    /// `vector-ref` callback declined.
    VectorRefDeclined = 49,
    /// A block no control flow reaches (after a tail `SelfCall`) — unreachable.
    UnreachableLeader = 50,
    /// `count` callback declined (a record, seq-view, rope, table, improper list, …).
    CountDeclined = 51,
    /// `(nth v i default)` callback declined (not a vector, or not a fixnum index).
    VectorRef3Declined = 52,
    /// Arithmetic dispatched by tag: the first operand is neither an int nor a float.
    ArithDispatchFirstOperand = 53,
    /// Arithmetic dispatched by tag: the second operand is neither an int nor a float.
    ArithDispatchSecondOperand = 54,
    /// A self-tail call passed a register-carried parameter a value of another type;
    /// payload = the slot.
    SelfCallCarryTag = 55,
    /// Native recursion past the depth cap: drain the subtree on the VM.
    NativeDepthExceeded = 101,
    /// Entry hoist: a vector slot's element base was null (not a vector).
    HoistedSlotVectorBaseNull = 102,
    /// Entry hoist: a global the arm reads is unbound.
    HoistedGlobalUnbound = 103,
    /// Entry hoist: a global vector's element base was null (not a vector).
    HoistedGlobalVectorBaseNull = 104,
    /// Entry hoist: a scalar global the arm reads is unbound.
    HoistedScalarGlobalUnbound = 105,
    /// A register-carried parameter's entry tag check failed; payload = the slot.
    EntryParamTag = 106,
    /// The entry reduction poll found a limit waiting to be raised, which only the VM can
    /// do (ADR-401) — the entry twin of `BackEdgeLimitPending`.
    EntryLimitPending = 107,
}

impl DeoptReason {
    /// Every reason, for the uniqueness test and for decoding.
    pub(crate) const ALL: &'static [DeoptReason] = &[
        DeoptReason::UnreachableDoneEdge,
        DeoptReason::DeadJoinBlock,
        DeoptReason::UntaggedIntBranch,
        DeoptReason::PairAccessorDeclined,
        DeoptReason::IsEmptyDeclined,
        DeoptReason::SqrtFloatNotPositive,
        DeoptReason::SqrtIntNotPositive,
        DeoptReason::SqrtBoxedFloatNotPositive,
        DeoptReason::SqrtOperandNotNumber,
        DeoptReason::SqrtBoxedIntNotPositive,
        DeoptReason::HoistedTablePutDeclined,
        DeoptReason::TablePutDeclined,
        DeoptReason::HoistedVectorIndexOutOfRange,
        DeoptReason::HoistedSlotVectorIndexOutOfRange,
        DeoptReason::IntArithmeticOverflow,
        DeoptReason::IntDivideByZero,
        DeoptReason::IntDivideOverflow,
        DeoptReason::IntDivideInexact,
        DeoptReason::FloatDivideByZero,
        DeoptReason::SlotIntGuard,
        DeoptReason::HandleIntGuard,
        DeoptReason::HoistedValueNotInt,
        DeoptReason::FloatWhereIntRequired,
        DeoptReason::SlotFloatGuard,
        DeoptReason::HandleFloatGuard,
        DeoptReason::NeverFloatOperand,
        DeoptReason::BoolWhereNumberRequired,
        DeoptReason::TableReadDeclined,
        DeoptReason::EqualSeqViewOperand,
        DeoptReason::InlineVectorRefNotVector,
        DeoptReason::InlineVectorRefOutOfRange,
        DeoptReason::InlineVectorRefUnexpectedStore,
        DeoptReason::InlineVectorRefSpilledOutOfRange,
        DeoptReason::InlineVectorRefDeclined,
        DeoptReason::BackEdgeEpochChanged,
        DeoptReason::BackEdgeLimitPending,
        DeoptReason::MapGet3Declined,
        DeoptReason::MapAssocDeclined,
        DeoptReason::VectorLengthNotVector,
        DeoptReason::FloatPairNeverFloat,
        DeoptReason::FloatPairRightBesideFloat,
        DeoptReason::FloatPairLeftBesideFloat,
        DeoptReason::FloatPairSecondBesideFloat,
        DeoptReason::FloatPairFirstNotNumber,
        DeoptReason::FloatPairSecondBesideInt,
        DeoptReason::CompareFirstOperand,
        DeoptReason::CompareSecondOperand,
        DeoptReason::VectorRefDeclined,
        DeoptReason::UnreachableLeader,
        DeoptReason::CountDeclined,
        DeoptReason::VectorRef3Declined,
        DeoptReason::ArithDispatchFirstOperand,
        DeoptReason::ArithDispatchSecondOperand,
        DeoptReason::SelfCallCarryTag,
        DeoptReason::NativeDepthExceeded,
        DeoptReason::HoistedSlotVectorBaseNull,
        DeoptReason::HoistedGlobalUnbound,
        DeoptReason::HoistedGlobalVectorBaseNull,
        DeoptReason::HoistedScalarGlobalUnbound,
        DeoptReason::EntryParamTag,
        DeoptReason::EntryLimitPending,
    ];

    /// The reason code with no payload — what a guard passes to the deopt block.
    pub(crate) const fn code(self) -> i64 {
        self as u8 as i64
    }

    /// The reason code carrying `payload` above the reason byte.
    pub(crate) const fn code_with(self, payload: i64) -> i64 {
        self.code() | (payload << 8)
    }

    /// Split a parked reason code into its reason and payload. `None` for a code no guard
    /// emits (0: nothing has deopted yet).
    pub(crate) fn decode(code: u32) -> (Option<DeoptReason>, u32) {
        let low = (code & 0xff) as u8;
        let reason = Self::ALL.iter().copied().find(|r| *r as u8 == low);
        (reason, code >> 8)
    }

    /// The kebab-case name the traces print.
    pub(crate) fn name(self) -> &'static str {
        match self {
            DeoptReason::UnreachableDoneEdge => "unreachable-done-edge",
            DeoptReason::DeadJoinBlock => "dead-join-block",
            DeoptReason::UntaggedIntBranch => "untagged-int-branch",
            DeoptReason::PairAccessorDeclined => "pair-accessor-declined",
            DeoptReason::IsEmptyDeclined => "is-empty-declined",
            DeoptReason::SqrtFloatNotPositive => "sqrt-float-not-positive",
            DeoptReason::SqrtIntNotPositive => "sqrt-int-not-positive",
            DeoptReason::SqrtBoxedFloatNotPositive => "sqrt-boxed-float-not-positive",
            DeoptReason::SqrtOperandNotNumber => "sqrt-operand-not-number",
            DeoptReason::SqrtBoxedIntNotPositive => "sqrt-boxed-int-not-positive",
            DeoptReason::HoistedTablePutDeclined => "hoisted-table-put-declined",
            DeoptReason::TablePutDeclined => "table-put-declined",
            DeoptReason::HoistedVectorIndexOutOfRange => "hoisted-vector-index-out-of-range",
            DeoptReason::HoistedSlotVectorIndexOutOfRange => {
                "hoisted-slot-vector-index-out-of-range"
            }
            DeoptReason::IntArithmeticOverflow => "int-arithmetic-overflow",
            DeoptReason::IntDivideByZero => "int-divide-by-zero",
            DeoptReason::IntDivideOverflow => "int-divide-overflow",
            DeoptReason::IntDivideInexact => "int-divide-inexact",
            DeoptReason::FloatDivideByZero => "float-divide-by-zero",
            DeoptReason::SlotIntGuard => "slot-int-guard",
            DeoptReason::HandleIntGuard => "handle-int-guard",
            DeoptReason::HoistedValueNotInt => "hoisted-value-not-int",
            DeoptReason::FloatWhereIntRequired => "float-where-int-required",
            DeoptReason::SlotFloatGuard => "slot-float-guard",
            DeoptReason::HandleFloatGuard => "handle-float-guard",
            DeoptReason::NeverFloatOperand => "never-float-operand",
            DeoptReason::BoolWhereNumberRequired => "bool-where-number-required",
            DeoptReason::TableReadDeclined => "table-read-declined",
            DeoptReason::EqualSeqViewOperand => "equal-seq-view-operand",
            DeoptReason::InlineVectorRefNotVector => "inline-vector-ref-not-vector",
            DeoptReason::InlineVectorRefOutOfRange => "inline-vector-ref-out-of-range",
            DeoptReason::InlineVectorRefUnexpectedStore => "inline-vector-ref-unexpected-store",
            DeoptReason::InlineVectorRefSpilledOutOfRange => {
                "inline-vector-ref-spilled-out-of-range"
            }
            DeoptReason::InlineVectorRefDeclined => "inline-vector-ref-declined",
            DeoptReason::BackEdgeEpochChanged => "back-edge-epoch-changed",
            DeoptReason::BackEdgeLimitPending => "back-edge-limit-pending",
            DeoptReason::MapGet3Declined => "map-get3-declined",
            DeoptReason::MapAssocDeclined => "map-assoc-declined",
            DeoptReason::VectorLengthNotVector => "vector-length-not-vector",
            DeoptReason::FloatPairNeverFloat => "float-pair-never-float",
            DeoptReason::FloatPairRightBesideFloat => "float-pair-right-beside-float",
            DeoptReason::FloatPairLeftBesideFloat => "float-pair-left-beside-float",
            DeoptReason::FloatPairSecondBesideFloat => "float-pair-second-beside-float",
            DeoptReason::FloatPairFirstNotNumber => "float-pair-first-not-number",
            DeoptReason::FloatPairSecondBesideInt => "float-pair-second-beside-int",
            DeoptReason::CompareFirstOperand => "compare-first-operand",
            DeoptReason::CompareSecondOperand => "compare-second-operand",
            DeoptReason::VectorRefDeclined => "vector-ref-declined",
            DeoptReason::UnreachableLeader => "unreachable-leader",
            DeoptReason::CountDeclined => "count-declined",
            DeoptReason::VectorRef3Declined => "vector-ref3-declined",
            DeoptReason::ArithDispatchFirstOperand => "arith-dispatch-first-operand",
            DeoptReason::ArithDispatchSecondOperand => "arith-dispatch-second-operand",
            DeoptReason::SelfCallCarryTag => "self-call-carry-tag",
            DeoptReason::NativeDepthExceeded => "native-depth-exceeded",
            DeoptReason::HoistedSlotVectorBaseNull => "hoisted-slot-vector-base-null",
            DeoptReason::HoistedGlobalUnbound => "hoisted-global-unbound",
            DeoptReason::HoistedGlobalVectorBaseNull => "hoisted-global-vector-base-null",
            DeoptReason::HoistedScalarGlobalUnbound => "hoisted-scalar-global-unbound",
            DeoptReason::EntryParamTag => "entry-param-tag",
            DeoptReason::EntryLimitPending => "entry-limit-pending",
        }
    }

    /// A parked reason code as the traces print it: the name, plus the payload when the
    /// reason carries one (`handle-int-guard(tag=3)`, `entry-param-tag(slot=1)`).
    pub(crate) fn describe(code: u32) -> String {
        match Self::decode(code) {
            (None, _) => format!("unknown#{code}"),
            (Some(reason @ (DeoptReason::SlotIntGuard | DeoptReason::HandleIntGuard)), tag) => {
                format!("{}(tag={tag})", reason.name())
            }
            (Some(reason @ (DeoptReason::EntryParamTag | DeoptReason::SelfCallCarryTag)), slot) => {
                format!("{}(slot={slot})", reason.name())
            }
            (Some(reason), _) => reason.name().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DeoptReason;

    /// Each guard site must be identifiable from its code alone: a reused id sends a
    /// `BROOD_DEOPT_TRACE` reader to the wrong guard. Also pins `ALL` against the enum — a
    /// variant missing from it would decode as unknown.
    #[test]
    fn every_reason_has_a_unique_code_and_name() {
        let mut codes: Vec<u8> = DeoptReason::ALL.iter().map(|r| *r as u8).collect();
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(before, codes.len(), "two deopt reasons share a code");
        assert!(
            !codes.contains(&0),
            "0 means 'no deopt yet' and must not be a reason"
        );
        let mut names: Vec<&str> = DeoptReason::ALL.iter().map(|r| r.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(before, names.len(), "two deopt reasons share a name");
        for reason in DeoptReason::ALL {
            assert_eq!(
                DeoptReason::decode(reason.code_with(7) as u32),
                (Some(*reason), 7),
                "{reason:?} does not round-trip through its code"
            );
        }
        // Exhaustiveness: a variant absent from `ALL` fails to compile here.
        for reason in DeoptReason::ALL {
            match reason {
                DeoptReason::UnreachableDoneEdge
                | DeoptReason::DeadJoinBlock
                | DeoptReason::UntaggedIntBranch
                | DeoptReason::PairAccessorDeclined
                | DeoptReason::IsEmptyDeclined
                | DeoptReason::SqrtFloatNotPositive
                | DeoptReason::SqrtIntNotPositive
                | DeoptReason::SqrtBoxedFloatNotPositive
                | DeoptReason::SqrtOperandNotNumber
                | DeoptReason::SqrtBoxedIntNotPositive
                | DeoptReason::HoistedTablePutDeclined
                | DeoptReason::TablePutDeclined
                | DeoptReason::HoistedVectorIndexOutOfRange
                | DeoptReason::HoistedSlotVectorIndexOutOfRange
                | DeoptReason::IntArithmeticOverflow
                | DeoptReason::IntDivideByZero
                | DeoptReason::IntDivideOverflow
                | DeoptReason::IntDivideInexact
                | DeoptReason::FloatDivideByZero
                | DeoptReason::SlotIntGuard
                | DeoptReason::HandleIntGuard
                | DeoptReason::HoistedValueNotInt
                | DeoptReason::FloatWhereIntRequired
                | DeoptReason::SlotFloatGuard
                | DeoptReason::HandleFloatGuard
                | DeoptReason::NeverFloatOperand
                | DeoptReason::BoolWhereNumberRequired
                | DeoptReason::TableReadDeclined
                | DeoptReason::EqualSeqViewOperand
                | DeoptReason::InlineVectorRefNotVector
                | DeoptReason::InlineVectorRefOutOfRange
                | DeoptReason::InlineVectorRefUnexpectedStore
                | DeoptReason::InlineVectorRefSpilledOutOfRange
                | DeoptReason::InlineVectorRefDeclined
                | DeoptReason::BackEdgeEpochChanged
                | DeoptReason::BackEdgeLimitPending
                | DeoptReason::MapGet3Declined
                | DeoptReason::MapAssocDeclined
                | DeoptReason::VectorLengthNotVector
                | DeoptReason::FloatPairNeverFloat
                | DeoptReason::FloatPairRightBesideFloat
                | DeoptReason::FloatPairLeftBesideFloat
                | DeoptReason::FloatPairSecondBesideFloat
                | DeoptReason::FloatPairFirstNotNumber
                | DeoptReason::FloatPairSecondBesideInt
                | DeoptReason::CompareFirstOperand
                | DeoptReason::CompareSecondOperand
                | DeoptReason::VectorRefDeclined
                | DeoptReason::UnreachableLeader
                | DeoptReason::CountDeclined
                | DeoptReason::VectorRef3Declined
                | DeoptReason::ArithDispatchFirstOperand
                | DeoptReason::ArithDispatchSecondOperand
                | DeoptReason::SelfCallCarryTag
                | DeoptReason::NativeDepthExceeded
                | DeoptReason::HoistedSlotVectorBaseNull
                | DeoptReason::HoistedGlobalUnbound
                | DeoptReason::HoistedGlobalVectorBaseNull
                | DeoptReason::HoistedScalarGlobalUnbound
                | DeoptReason::EntryParamTag
                | DeoptReason::EntryLimitPending => {}
            }
        }
    }

    /// The lowering may not hand a raw number to a deopt block: every guard's reason value
    /// must come from one of `emit::deopt_code*` (typed `DeoptReason`), or the uniqueness
    /// above guarantees nothing about what is emitted.
    #[test]
    fn no_guard_passes_a_raw_reason_number() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/eval/compile");
        let mut stack = vec![root];
        let mut offenders = Vec::new();
        let mut guards = 0;
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)
                .expect("read the compile tree")
                .flatten()
            {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                // This file names the pattern it searches for.
                if path.file_name().is_some_and(|n| n == "deopt_reason.rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (offset, _) in text.match_indices("let deopt_reason =") {
                    guards += 1;
                    let statement = &text[offset..];
                    let statement = &statement[..statement.find(';').unwrap_or(statement.len())];
                    // `deopt_code`, `deopt_code_with`, `deopt_code_with_constant`.
                    if !statement.contains("deopt_code") {
                        let line_number = text[..offset].lines().count();
                        offenders.push(format!("{}:{line_number}", path.display()));
                    }
                }
            }
        }
        assert!(
            guards > 30,
            "found only {guards} deopt guards — wrong root?"
        );
        assert!(
            offenders.is_empty(),
            "a deopt guard passes a raw reason number instead of a `DeoptReason`: {offenders:?}"
        );
    }
}
