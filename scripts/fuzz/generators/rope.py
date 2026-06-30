#!/usr/bin/env python3
"""Rope property fuzzer. A rope must ALWAYS equal the plain string it represents.
Apply a random sequence of insert/delete edits to both a rope (in Brood, threaded
via `->`) and a reference Python string; assert (rope->string r) == reference,
plus rope-length and a sample rope-slice. Codepoints align (both index by
codepoint). Includes newlines + unicode. Each program prints OK / BAD.
"""
import random, sys

ALPHA = list("abcde \n\nXY") + ["é", "ö", "λ", "🙂"]

def blit(s):
    # Brood string literal for s (escape \, ", newline)
    out = s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")
    return '"' + out + '"'

def rand_str(rng, lo, hi):
    return "".join(rng.choice(ALPHA) for _ in range(rng.randint(lo, hi)))

def program(seed):
    rng = random.Random(seed)
    init = rand_str(rng, 0, 12)
    ref = init
    ops = []                       # Brood threading steps
    for _ in range(rng.randint(3, 25)):
        if not ref or rng.random() < 0.55:        # insert
            pos = rng.randint(0, len(ref))
            ins = rand_str(rng, 1, 5)
            ops.append(f"(rope-insert {pos} {blit(ins)})")
            ref = ref[:pos] + ins + ref[pos:]
        else:                                      # delete [a, b)
            a = rng.randint(0, len(ref) - 1)
            b = rng.randint(a + 1, len(ref))
            ops.append(f"(rope-delete {a} {b})")
            ref = ref[:a] + ref[b:]
    # sample slice oracle
    if ref:
        sa = rng.randint(0, len(ref) - 1); sb = rng.randint(sa, len(ref))
    else:
        sa = sb = 0
    chain = "(-> (string->rope " + blit(init) + ")\n      " + "\n      ".join(ops) + ")"
    return (f"(let (r {chain})\n"
            f"  (let (s (rope->string r))\n"
            f"    (println (if (and (= s {blit(ref)})\n"
            f"                      (= (rope-length r) {len(ref)})\n"
            f"                      (= (rope-slice r {sa} {sb}) {blit(ref[sa:sb])}))\n"
            f"               \"OK\" (str \"BAD len=\" (rope-length r) \" s=\" (pr-str s))))))\n")

if __name__ == "__main__":
    n = int(sys.argv[1]); base = int(sys.argv[2]); outdir = sys.argv[3]
    for k in range(n):
        seed = base + k
        with open(f"{outdir}/rp_{seed}.blsp", "w") as fh:
            fh.write(program(seed))
    print(f"wrote {n} rope programs")
