# Python oracle for stress/table_digest.blsp — identical LCG + op sequence.
d = {}
s = 20260716
for _ in range(200000):
    s = (s * 1103515245 + 12345) % 2147483648
    c = s % 10
    k = s % 4096
    v = s % 100000
    if c < 6:
        d[k] = v
    elif c < 8:
        d.pop(k, None)
cnt = len(d)
skv = sum(k * v for k, v in d.items())
sk = sum(d.keys())
print("digest", cnt, skv, sk, len(d))
