#!/usr/bin/env python3
"""Reader / evaluator ROBUSTNESS fuzzer — adversarial, mostly-invalid input.

The differential fuzzer (`fuzz_programs.py`) only generates VALID, terminating
programs, so it can never reach the reader's and evaluator's error paths on
malformed input. This one does the opposite: it feeds `brood` a stream of
deliberately hostile inputs — random bytes, unbalanced/blown-out delimiters,
truncated strings and escapes, giant tokens, weird numerics, deep nesting — and
asserts the process always FAILS GRACEFULLY:

  * never a crash SIGNAL (segfault 139 / abort 134 / bus 135) — a parser or
    evaluator memory fault,
  * never a Rust PANIC (`.brood_crash_dump` written, or "panicked at" on stderr)
    — an unwrap/index-out-of-bounds the reader should have handled as an error,
  * never a HANG (must exit within the timeout) — an infinite loop on malformed
    input.

A clean nonzero exit with a diagnostic is the CORRECT outcome for bad input and
is not a finding. Any signal / panic / timeout is kept for triage.

Usage: python3 stress/fuzz_reader.py [--seeds N] [--start S]
"""
import argparse, os, random, subprocess, sys

BROOD = os.environ.get("BROOD", "target/release/brood")

# printable-ish soup plus the structural characters that drive the reader
SOUP = (list("()[]{}\"\\;'`,~#:@ \n\t")
        + list("abcdefghijklmnopqrstuvwxyz0123456789")
        + list("+-*/<>=!?._|&^%$")
        + ["\\n", "\\t", "\\\"", "\\x", "\\u", "1e", "0x", ".", "..", "...",
           "1/0", "1M", "1.5e999", "-", "+inf", "nan", ":", "::", "#(", "#{",
           "'", "`", ",@", "~@"])

def gen(r):
    """One adversarial input, biased toward structurally-interesting garbage."""
    mode = r.random()
    if mode < 0.25:
        # raw random bytes (may be invalid UTF-8 — the reader must not choke)
        n = r.randint(0, 400)
        return bytes(r.randint(0, 255) for _ in range(n))
    if mode < 0.45:
        # unbalanced / blown-out delimiters
        opens = r.choice("([{")
        return (opens * r.randint(1, 2000)).encode()
    if mode < 0.60:
        # deeply nested but balanced — stack-depth stress on the reader
        d = r.randint(1, 5000)
        return (("(" * d) + ("1 " * r.randint(0, 5)) + (")" * d)).encode()
    if mode < 0.72:
        # truncated string / escape
        s = '"' + "".join(r.choice(SOUP) for _ in range(r.randint(0, 60)))
        if r.random() < 0.5:
            s += "\\"                      # dangling escape at EOF
        return s.encode()
    if mode < 0.82:
        # hostile numerics / reader macros
        toks = [r.choice(["1e999999", "0x", "1/0", "-", "1.2.3", "##", "#:",
                          ".5.5", "1M2", "0b", "1e", "+", ":", "'", "`", ",@"])
                for _ in range(r.randint(1, 40))]
        return (" ".join(toks)).encode()
    if mode < 0.92:
        # a giant single token (symbol/number/string) — no delimiters
        c = r.choice("a1\"\\;:")
        return (c * r.randint(1000, 20000)).encode()
    # structured soup: random draws from SOUP, sometimes wrapped in a form
    parts = [r.choice(SOUP) for _ in range(r.randint(1, 200))]
    s = "".join(parts)
    if r.random() < 0.4:
        s = "(" + s + ")"
    return s.encode()

CRASH_SIGNALS = {134: "SIGABRT", 135: "SIGBUS", 136: "SIGFPE", 139: "SIGSEGV",
                 132: "SIGILL", 133: "SIGTRAP"}

def classify(inp, tmp, workdir, binpath):
    with open(tmp, "wb") as fh:
        fh.write(inp)
    # the panic hook writes `.brood_crash_dump` to the process CWD — run in a
    # dedicated workdir so a concurrent differential sweep can't cross-pollute it.
    dump = os.path.join(workdir, ".brood_crash_dump")
    try:
        os.remove(dump)
    except OSError:
        pass
    env = dict(os.environ)
    env["BROOD_NO_CHECK"] = "1"
    try:
        out = subprocess.run([binpath, os.path.abspath(tmp)], capture_output=True,
                             timeout=20, env=env, cwd=workdir)
    except subprocess.TimeoutExpired:
        return ("HANG", "no exit within 20s")
    rc = out.returncode
    if rc < 0 and (-rc) in CRASH_SIGNALS:
        return ("SIGNAL", CRASH_SIGNALS[-rc])
    if rc in CRASH_SIGNALS:  # some shells surface 128+sig
        return ("SIGNAL", CRASH_SIGNALS[rc])
    stderr = out.stderr.decode("utf-8", "replace")
    if "panicked at" in stderr or os.path.exists(dump):
        first = next((l for l in stderr.splitlines() if "panicked at" in l), "crash dump written")
        return ("PANIC", first[:200])
    return (None, None)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seeds", type=int, default=2000)
    ap.add_argument("--start", type=int, default=1)
    args = ap.parse_args()
    outdir = "stress/fuzz_out"
    os.makedirs(outdir, exist_ok=True)
    workdir = os.path.join(outdir, "reader_work")
    os.makedirs(workdir, exist_ok=True)
    tmp = os.path.join(workdir, "reader_probe.blsp")
    binpath = os.path.abspath(BROOD)
    bad = 0
    for seed in range(args.start, args.start + args.seeds):
        r = random.Random(seed)
        inp = gen(r)
        kind, detail = classify(inp, tmp, workdir, binpath)
        if kind is not None:
            bad += 1
            keep = os.path.join(outdir, f"reader_{kind}_{seed}.bin")
            with open(keep, "wb") as fh:
                fh.write(inp)
            print(f"{kind} seed={seed} ({detail}) — kept {keep} ({len(inp)} bytes)")
        elif seed % 500 == 0:
            sys.stdout.write(f"seed {seed} ok\n"); sys.stdout.flush()
    try:
        os.remove(tmp)
    except OSError:
        pass
    if bad:
        print(f"---- reader-fuzz: {bad}/{args.seeds} inputs crashed/panicked/hung")
        return 1
    print(f"---- reader-fuzz: {args.seeds} adversarial inputs, all failed gracefully")
    return 0

if __name__ == "__main__":
    sys.exit(main())
