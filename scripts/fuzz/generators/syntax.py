#!/usr/bin/env python3
"""Silly-syntax fuzzer: emit adversarial/malformed Brood source. The reader must
NEVER panic/segfault — only return a clean parse error. We also throw in
valid-but-weird forms to stress the scanner/number-classifier/quote-sugar paths.
"""
import random, sys

# tokens chosen to stress delimiters, number shapes, sugar, strings, comments
TOKS = [
    "(", ")", "[", "]", "{", "}", "'", "`", "~", "~@", "#", "##", "#b", "#;",
    "#|", "|#", ";comment\n", ".", "..", "...", ":", "::", ":kw", "nil", "true",
    "false", "inf", "-inf", "nan", "+", "-", "*", "/", "=",
    # number-ish (many invalid): exponents, signs, radixes, separators, decimals
    "1", "-1", "1.5", "-1.5", "1e", "1e9", "1e999999", "1.2.3", "1..2", "--5",
    "0x1F", "0b101", "0o77", "1_000", "1/0", "1/2", ".5", "5.", "1M", "1.5M",
    "1mm", "0000", "9" * 40, "1e-", "+.", "-.", "1ee9", "0x", "1.0e308", "1e400",
    # strings (many unterminated / bad escapes)
    '"hi"', '"unterminated', '"bad\\x"', '"\\u{ZZZZ}"', '"\\"', '"\\u{110000}"',
    '"\\e\\0\\n\\t"', '""', '"' + "a" * 50 + '"',
    # symbols with odd chars
    "foo", "foo-bar", "a/b", "/", "->", "<=>", "...x", "a b", "café", "λ",
    # whitespace / commas
    " ", ",", "\t", "\n",
]

# occasional raw bytes to stress UTF-8 decoding in the scanner
def junk_byte(rng):
    return rng.choice(["\x00", "\x01", "\x1b", "\x7f", "\\", "@", "%", "&", "^",
                       "\xc3\x28", "\xed\xa0\x80"])  # invalid/partial UTF-8 seqs

def program(seed):
    rng = random.Random(seed)
    n = rng.randint(1, 60)
    parts = []
    for _ in range(n):
        r = rng.random()
        if r < 0.78:
            parts.append(rng.choice(TOKS))
        elif r < 0.9:
            parts.append(junk_byte(rng))
        else:
            # a run of one delimiter to stress nesting / balance
            parts.append(rng.choice(["(", ")", "[", "]", "{", "}", "'", "~"]) * rng.randint(1, 40))
    sep = rng.choice(["", " ", " ", "\n"])
    return sep.join(parts)

if __name__ == "__main__":
    n = int(sys.argv[1]); base = int(sys.argv[2]); outdir = sys.argv[3]
    for k in range(n):
        seed = base + k
        data = program(seed)
        # write as bytes (latin-1 so our raw \xNN survive verbatim)
        with open(f"{outdir}/sx_{seed}.blsp", "wb") as fh:
            fh.write(data.encode("latin-1", "replace"))
    print(f"wrote {n} syntax programs")
