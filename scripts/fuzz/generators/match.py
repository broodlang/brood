#!/usr/bin/env python3
"""Pattern-match fuzzer with a value-derived oracle. Generate a random nested
value V, derive a pattern P from V (each node: keep literal | wildcard `_` | bind),
so we KNOW match must succeed and exactly what each binder gets. Emit a
self-checking program that prints OK / BAD. A second clause mutates one literal so
the pattern must FAIL (fall to the catch-all). Same program runs under both
engines; both must print OK.
"""
import random, sys

class Node:
    def __init__(self, kind, val=None, kids=None):
        self.kind, self.val, self.kids = kind, val, kids or []

def gen_value(rng, depth):
    if depth <= 0 or rng.random() < 0.4:
        k = rng.choice(["int", "kw", "str", "bool", "nil"])
        if k == "int":  return Node("int", rng.choice([0, 1, -1, 2, 7, 42, -5]))
        if k == "kw":   return Node("kw", rng.choice(["a", "b", "ok", "err", "x"]))
        if k == "str":  return Node("str", rng.choice(["", "hi", "yo", "z"]))
        if k == "bool": return Node("bool", rng.choice([True, False]))
        return Node("nil")
    kind = rng.choice(["vec", "list"])
    n = rng.randint(0, 3)
    return Node(kind, kids=[gen_value(rng, depth-1) for _ in range(n)])

def render_value(n):
    if n.kind == "int":  return str(n.val)
    if n.kind == "kw":   return ":" + n.val
    if n.kind == "str":  return '"' + n.val + '"'
    if n.kind == "bool": return "true" if n.val else "false"
    if n.kind == "nil":  return "nil"
    if n.kind == "vec":  return "[" + " ".join(render_value(k) for k in n.kids) + "]"
    if n.kind == "list": return "(list " + " ".join(render_value(k) for k in n.kids) + ")"

# derive a pattern; append (binder_name, value_node) to `binds` in pre-order
def render_pattern(rng, n, binds, ctr):
    r = rng.random()
    if r < 0.33:                              # bind the whole sub-value
        name = f"g{ctr[0]}"; ctr[0] += 1
        binds.append((name, n))
        return name
    if r < 0.5:                               # wildcard
        return "_"
    # else: keep structure (literal leaf, or recurse into a container)
    if n.kind in ("vec", "list"):
        inner = " ".join(render_pattern(rng, k, binds, ctr) for k in n.kids)
        return ("[" + inner + "]") if n.kind == "vec" else ("(" + inner + ")")
    return render_value(n)                    # literal leaf

def first_literal_mutation(n):
    # return a value-node text that differs, for the non-match clause
    if n.kind == "int":  return str(n.val + 1000)
    if n.kind == "kw":   return ":zzz_no"
    if n.kind == "str":  return '"NOPE_xyz"'
    if n.kind == "bool": return "false" if n.val else "true"
    if n.kind == "nil":  return ":not-nil"
    return None

def program(seed):
    rng = random.Random(seed)
    v = gen_value(rng, 3)
    vtext = render_value(v)
    binds = []; ctr = [0]
    ptext = render_pattern(rng, v, binds, ctr)
    if binds:
        body = "(list " + " ".join(name for name, _ in binds) + ")"
        expected = "(list " + " ".join(render_value(node) for _, node in binds) + ")"
    else:
        body, expected = ":MATCHED", ":MATCHED"
    out = []
    out.append(f"(let (got (match {vtext} ({ptext} {body}) (_ :NOMATCH)))")
    out.append(f"  (println (if (= got {expected}) \"OK\" (str \"BAD-match got=\" (pr-str got)))))")
    # non-match clause: mutate the value's first leaf literal in the pattern so it can't match.
    # We mutate the VALUE instead (simpler+sound): a value that differs at a kept literal.
    # Use a guaranteed-non-matching literal pattern: match the original value against a
    # different scalar pattern; it must fall through.
    diff = first_literal_mutation(v) if v.kind not in ("vec", "list") else None
    if diff:
        out.append(f"(println (if (= :NOMATCH (match {vtext} ({diff} :HIT) (_ :NOMATCH))) \"OK\" \"BAD-nomatch\"))")
    return "\n".join(out) + "\n"

if __name__ == "__main__":
    n = int(sys.argv[1]); base = int(sys.argv[2]); outdir = sys.argv[3]
    for k in range(n):
        seed = base + k
        with open(f"{outdir}/mt_{seed}.blsp", "w") as fh:
            fh.write(program(seed))
    print(f"wrote {n} match programs")
