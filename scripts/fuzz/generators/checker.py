#!/usr/bin/env python3
"""Type-checker crash-resistance fuzzer. The advisory checker must NEVER panic or
hang on any form — it returns a (possibly empty) warning list. Generate forms with
random AND malformed type annotations (sigs) + bodies, run them through
`(check 'form)`, and assert it returns without crashing or hanging.
"""
import random, sys

BASE = ["any", "never", "int", "float", "number", "string", "symbol", "bool", "nil",
        "keyword", "list", "vector", "map"]
def ty(rng, d):
    if d <= 0 or rng.random() < 0.4:
        r = rng.random()
        if r < 0.5: return rng.choice(BASE)
        if r < 0.7: return ":" + rng.choice(["a", "b", "ok", "x"])     # literal keyword
        if r < 0.85: return "?" + rng.choice(["A", "B", "el"])          # typevar
        return rng.choice(["foo", "Unknown", "123", "()"])             # garbage/unknown
    r = rng.random()
    if r < 0.2:  return f"({ty(rng,d-1)} {ty(rng,d-1)} -> {ty(rng,d-1)})"   # arrow
    if r < 0.35: return f"(list {ty(rng,d-1)})"
    if r < 0.5:  return f"(vector {ty(rng,d-1)})"
    if r < 0.6:  return f"(map {ty(rng,d-1)} {ty(rng,d-1)})"
    if r < 0.75: return f"(or {ty(rng,d-1)} {ty(rng,d-1)})"
    if r < 0.85: return f"(and {ty(rng,d-1)} {ty(rng,d-1)})"
    # malformed: empty/under-arity/bad arrows
    return rng.choice(["(or)", "(and)", "(list)", "(map int)", "(-> int)",
                       f"(int -> )", f"({ty(rng,d-1)} ->)", "(or int int int int int)",
                       f"(and {ty(rng,d-1)})"])

def body(rng, d):
    if d <= 0 or rng.random() < 0.5:
        return rng.choice(["x", "1", "1.5", ":k", "\"s\"", "nil", "true", "[]", "{}"])
    op = rng.choice(["+", "-", "*", "if", "let", "cons", "str", "vector"])
    if op == "if":  return f"(if {body(rng,d-1)} {body(rng,d-1)} {body(rng,d-1)})"
    if op == "let": return f"(let (y {body(rng,d-1)}) {body(rng,d-1)})"
    return f"({op} {body(rng,d-1)} {body(rng,d-1)})"

def program(seed):
    rng = random.Random(seed)
    arr = f"({ty(rng,3)} -> {ty(rng,3)})"
    form = f"(do (sig f {arr}) (defn f (x) {body(rng,3)}))"
    # `check` must return (a list) without crashing; print a marker
    return (f"(println (try (do (check (quote {form})) \"OK\")\n"
            f"  (catch e (str \"CAUGHT \" (get e :message)))))\n")

if __name__ == "__main__":
    n = int(sys.argv[1]); base = int(sys.argv[2]); outdir = sys.argv[3]
    for k in range(n):
        seed = base + k
        with open(f"{outdir}/ck_{seed}.blsp", "w") as fh:
            fh.write(program(seed))
    print(f"wrote {n} checker programs")
