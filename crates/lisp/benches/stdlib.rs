//! Benchmarks for opt-in `std/` modules — the library surface beyond the
//! prelude. Each bench requires its module inline and uses `bench_prog` so
//! the prelude build stays outside the timed region.
//!
//! Run with `cargo bench --bench stdlib` or `make benchmark`.
//! Results are archived by `scripts/bench.sh` like the other benches.

use brood::Interp;

fn main() {
    divan::main();
}

fn bench_prog(bencher: divan::Bencher, src: String) {
    bencher
        .with_inputs(Interp::new)
        .bench_refs(|interp| interp.eval_str(&src).unwrap());
}

// ---------------------------------------------------------------------------
// path
// ---------------------------------------------------------------------------
mod path {
    use super::*;

    /// Join 4-segment paths `n` times.
    #[divan::bench(args = [100, 1_000])]
    fn join(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'path) \
                 (dotimes (i {n}) (path/join \"/usr\" \"local\" \"lib\" \"brood\"))"
            ),
        );
    }

    /// Normalize a path with `.` and `..` `n` times.
    #[divan::bench(args = [100, 1_000])]
    fn normalize(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'path) \
                 (dotimes (i {n}) (path/normalize \"/usr/local/lib/../share/./brood\"))"
            ),
        );
    }

    /// `basename` + `dirname` + `extension` + `stem` on the same path.
    #[divan::bench(args = [100, 1_000])]
    fn decompose(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'path) \
                 (dotimes (i {n}) \
                   (do (path/basename  \"/usr/local/lib/brood.tar.gz\") \
                       (path/dirname   \"/usr/local/lib/brood.tar.gz\") \
                       (path/extension \"/usr/local/lib/brood.tar.gz\") \
                       (path/stem      \"/usr/local/lib/brood.tar.gz\")))"
            ),
        );
    }

    /// `relative-to`: compute relative path between two absolute paths.
    #[divan::bench(args = [100, 1_000])]
    fn relative_to(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'path) \
                 (dotimes (i {n}) (path/relative-to \"/a/b/x/y/z\" \"/a/b/c/d\"))"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// encoding  (hex / base64)
// ---------------------------------------------------------------------------
mod encoding {
    use super::*;

    // 64-character ASCII string (deterministic, no escaping needed in Brood source).
    const STR_64: &str = "\"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWX\"";

    /// Hex-encode a 64-character string `n` times.
    #[divan::bench(args = [100, 1_000])]
    fn hex_encode(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'encoding) \
                 (def s {STR_64}) \
                 (dotimes (i {n}) (encoding/hex-encode s))"
            ),
        );
    }

    /// Hex round-trip: encode then decode `n` times.
    #[divan::bench(args = [100, 1_000])]
    fn hex_roundtrip(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'encoding) \
                 (def s {STR_64}) \
                 (dotimes (i {n}) (encoding/hex-decode (encoding/hex-encode s)))"
            ),
        );
    }

    /// Base64-encode a 64-character string `n` times.
    #[divan::bench(args = [100, 1_000])]
    fn base64_encode(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'encoding) \
                 (def s {STR_64}) \
                 (dotimes (i {n}) (encoding/base64-encode s))"
            ),
        );
    }

    /// Base64 round-trip `n` times.
    #[divan::bench(args = [100, 1_000])]
    fn base64_roundtrip(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'encoding) \
                 (def s {STR_64}) \
                 (dotimes (i {n}) (encoding/base64-decode (encoding/base64-encode s)))"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// hash
// ---------------------------------------------------------------------------
mod hash {
    use super::*;

    /// SHA-256 a short string `n` times.
    #[divan::bench(args = [100, 500])]
    fn sha256_short(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'hash) \
                 (dotimes (i {n}) (hash/sha256 \"hello, brood!\"))"
            ),
        );
    }

    /// SHA-256 a 1 KiB string `n` times.
    #[divan::bench(args = [100])]
    fn sha256_1k(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'hash) \
                 (def s (string-repeat \"a\" 1024)) \
                 (dotimes (i {n}) (hash/sha256 s))"
            ),
        );
    }

    /// HMAC-SHA-256 a short string `n` times.
    #[divan::bench(args = [50, 200])]
    fn hmac_sha256(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'hash) \
                 (dotimes (i {n}) (hash/hmac-sha256 \"secret-key\" \"hello, brood!\"))"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// uuid
// ---------------------------------------------------------------------------
mod uuid {
    use super::*;

    /// Generate `n` random UUID v4 strings.
    #[divan::bench(args = [100, 1_000])]
    fn v4(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'uuid) \
                 (dotimes (i {n}) (uuid/uuid-v4))"
            ),
        );
    }

    /// Generate `n` time-ordered UUID v7 strings.
    #[divan::bench(args = [100, 1_000])]
    fn v7(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'uuid) \
                 (dotimes (i {n}) (uuid/uuid-v7))"
            ),
        );
    }

    /// Generate `n` name-based UUID v5 strings.
    #[divan::bench(args = [50, 200])]
    fn v5(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'uuid) \
                 (dotimes (i {n}) (uuid/uuid-v5 uuid/ns-dns \"example.com\"))"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// crypto
// ---------------------------------------------------------------------------
mod crypto {
    use super::*;

    /// Encrypt + decrypt a 64-byte payload `n` times (round-trip).
    #[divan::bench(args = [100])]
    fn chacha20_roundtrip(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'crypto) \
                 (def key   (crypto/random-key)) \
                 (def nonce (crypto/random-nonce)) \
                 (def plain (into [] (range 64))) \
                 (dotimes (i {n}) \
                   (crypto/decrypt key nonce (crypto/encrypt key nonce plain)))"
            ),
        );
    }

    /// PBKDF2-SHA256 at 1000 iterations `n` times.
    #[divan::bench(args = [5])]
    fn pbkdf2_1k_iter(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'crypto) \
                 (dotimes (i {n}) (crypto/pbkdf2 \"password\" \"salt\" 1000 32))"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// datetime
// ---------------------------------------------------------------------------
mod datetime {
    use super::*;

    /// Parse an ISO date string `n` times.
    #[divan::bench(args = [100, 1_000])]
    fn parse(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'datetime) \
                 (dotimes (i {n}) (datetime/parse-date \"2026-06-08\"))"
            ),
        );
    }

    /// `dt-add` 30 days (in ms) to a date `n` times.
    #[divan::bench(args = [100, 1_000])]
    fn add_days(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'datetime) \
                 (def d (datetime/date 2026 1 1)) \
                 (dotimes (i {n}) (datetime/dt-add d (datetime/days 30)))"
            ),
        );
    }

    /// `dt-format` a date `n` times.
    #[divan::bench(args = [100, 1_000])]
    fn format_date(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'datetime) \
                 (def d (datetime/date 2026 6 8)) \
                 (dotimes (i {n}) (datetime/dt-format d \"%Y-%m-%d\"))"
            ),
        );
    }

    /// `dt->epoch-ms` and back `n` times.
    #[divan::bench(args = [100, 1_000])]
    fn epoch_roundtrip(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'datetime) \
                 (def d (datetime/date 2026 6 8)) \
                 (dotimes (i {n}) (datetime/epoch-ms->dt (datetime/dt->epoch-ms d)))"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// csv
// ---------------------------------------------------------------------------
mod csv {
    use super::*;

    fn csv_prog(bencher: divan::Bencher, n: usize, body: &str) {
        let rows: Vec<String> = (0..n)
            .map(|i| format!("{i},name_{i},{}", i * 100))
            .collect();
        let csv = format!("id,name,score\n{}", rows.join("\n"));
        bench_prog(bencher, format!("(require 'csv) (def src {csv:?}) {body}"));
    }

    /// Parse a CSV string with `n` rows.
    #[divan::bench(args = [10, 100])]
    fn parse(bencher: divan::Bencher, n: usize) {
        csv_prog(bencher, n, "(csv/csv-parse src)");
    }

    /// Emit `n` rows as CSV (pre-parsed).
    #[divan::bench(args = [10, 100])]
    fn emit(bencher: divan::Bencher, n: usize) {
        csv_prog(bencher, n, "(csv/csv-emit (csv/csv-parse src))");
    }
}

// ---------------------------------------------------------------------------
// url
// ---------------------------------------------------------------------------
mod url {
    use super::*;

    /// `parse-url` `n` times.
    #[divan::bench(args = [100, 1_000])]
    fn parse(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'url) \
                 (dotimes (i {n}) \
                   (url/parse-url \"https://example.com/path/to/page?foo=bar&baz=qux#section\"))"
            ),
        );
    }

    /// `percent-encode` a string with special chars `n` times.
    #[divan::bench(args = [100, 1_000])]
    fn percent_encode(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'url) \
                 (dotimes (i {n}) (url/percent-encode \"hello world! foo=bar&baz=qux\"))"
            ),
        );
    }

    /// `query-encode` → `query-decode` round-trip `n` times.
    #[divan::bench(args = [50, 200])]
    fn query_roundtrip(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'url) \
                 (def q {{:name \"Alice Doe\" :age \"30\" :city \"New York\"}}) \
                 (dotimes (i {n}) (url/query-decode (url/query-encode q)))"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// stats
// ---------------------------------------------------------------------------
mod stats {
    use super::*;

    /// `mean` + `stddev` over `n` numbers.
    #[divan::bench(args = [100, 1_000])]
    fn mean_stddev(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'stats) \
                 (def xs (range {n})) \
                 (do (stats/mean xs) (stats/stddev xs))"
            ),
        );
    }

    /// `median` over `n` shuffled numbers (requires sort).
    #[divan::bench(args = [100, 1_000])]
    fn median(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'stats) \
                 (def xs (map (fn (x) (rem (* x 7919) {n})) (range {n}))) \
                 (stats/median xs)"
            ),
        );
    }

    /// `percentile` (p95) over `n` numbers.
    #[divan::bench(args = [100, 1_000])]
    fn percentile_95(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'stats) \
                 (def xs (range {n})) \
                 (stats/percentile xs 95)"
            ),
        );
    }

    /// `frequencies` over `n` numbers with 10 distinct keys.
    #[divan::bench(args = [100, 1_000])]
    fn frequencies(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'stats) \
                 (def xs (map (fn (x) (rem x 10)) (range {n}))) \
                 (stats/frequencies xs)"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// template
// ---------------------------------------------------------------------------
mod template {
    use super::*;

    /// Render a 4-variable template `n` times.
    #[divan::bench(args = [100, 1_000])]
    fn render(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            // Double-brace escaping: {{ → { in the format! string, so we need
            // 4 braces to produce the {{name}} template syntax.
            format!(
                "(require 'template) \
                 (def tmpl \"Hello, {{{{name}}}}! You have {{{{count}}}} messages.\") \
                 (def ctx {{:name \"Alice\" :count \"5\"}}) \
                 (dotimes (i {n}) (template/render tmpl ctx))"
            ),
        );
    }

    /// `render-all` over 10 contexts `n` times.
    #[divan::bench(args = [100])]
    fn render_all(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'template) \
                 (def tmpl \"Hello, {{{{name}}}}!\") \
                 (def ctxs (map (fn (i) {{:name (str \"User\" i)}}) (range 10))) \
                 (dotimes (i {n}) (template/render-all tmpl ctxs))"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------
mod diff {
    use super::*;

    /// LCS diff two lists of `n` integers with ~10% mutations.
    #[divan::bench(args = [50, 200])]
    fn diff_seq(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'diff) \
                 (def a (range {n})) \
                 (def b (filter (fn (x) (not= (rem x 10) 0)) (range {n}))) \
                 (diff/diff-seq a b)"
            ),
        );
    }

    /// Line diff of an `n`-line file with every 5th line changed.
    #[divan::bench(args = [20, 100])]
    fn diff_lines(bencher: divan::Bencher, n: usize) {
        let orig: String = (0..n).map(|i| format!("line {i}\n")).collect();
        let changed: String = (0..n)
            .map(|i| {
                if i % 5 == 0 {
                    format!("CHANGED {i}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect();
        bench_prog(
            bencher,
            format!(
                "(require 'diff) \
                 (diff/diff-lines {orig:?} {changed:?})"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// queue
// ---------------------------------------------------------------------------
mod queue {
    use super::*;

    /// Enqueue `n` items then dequeue all, summing values.
    #[divan::bench(args = [100, 1_000])]
    fn enqueue_dequeue(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'queue) \
                 (defn fill (q k) \
                   (if (= k 0) q \
                     (fill (queue/queue-push q k) (- k 1)))) \
                 (defn drain (q acc) \
                   (if (queue/queue-empty? q) acc \
                     (let (r (queue/queue-pop q)) \
                       (drain (second r) (+ acc (first r)))))) \
                 (drain (fill (queue/queue-new) {n}) 0)"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// multimap
// ---------------------------------------------------------------------------
mod multimap {
    use super::*;

    /// Add `n` key-value pairs (10 keys), then retrieve all values for key 0.
    #[divan::bench(args = [100, 1_000])]
    fn add_and_get(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'multimap) \
                 (def mm (fold (fn (m i) \
                                 (multimap/multimap-assoc m (rem i 10) i)) \
                               (multimap/multimap-new) \
                               (range {n}))) \
                 (multimap/multimap-get mm 0)"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// agent
// ---------------------------------------------------------------------------
mod agent {
    use super::*;

    /// Sequential: increment counter `n` times via `update`, then read with `get`.
    #[divan::bench(args = [50, 200])]
    fn update_get(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'agent) \
                 (def a (agent/start (fn () 0))) \
                 (dotimes (i {n}) (agent/update a inc)) \
                 (agent/get a identity) \
                 (agent/stop a)"
            ),
        );
    }

    /// `get-and-update` swap `n` times (atomic read+write).
    #[divan::bench(args = [50, 200])]
    fn get_and_update(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(require 'agent) \
                 (def a (agent/start (fn () 0))) \
                 (dotimes (i {n}) (agent/get-and-update a (fn (s) [s (+ s 1)]))) \
                 (agent/stop a)"
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// prelude enum extras
// ---------------------------------------------------------------------------
mod enum_extras {
    use super::*;

    /// `chunk-every` into groups of 10 over `n` elements.
    #[divan::bench(args = [1_000, 10_000])]
    fn chunk_every(bencher: divan::Bencher, n: usize) {
        bench_prog(bencher, format!("(count (chunk-every 10 (range {n})))"));
    }

    /// `chunk-by even?` over `n` integers.
    #[divan::bench(args = [1_000, 10_000])]
    fn chunk_by(bencher: divan::Bencher, n: usize) {
        bench_prog(bencher, format!("(count (chunk-by even? (range {n})))"));
    }

    /// Running sum via `scan` over `n` integers.
    #[divan::bench(args = [1_000, 10_000])]
    fn scan(bencher: divan::Bencher, n: usize) {
        bench_prog(bencher, format!("(last (scan + 0 (range {n})))"));
    }

    /// `zip-with +` two `n`-element ranges.
    #[divan::bench(args = [1_000, 10_000])]
    fn zip_with(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!("(last (zip-with + (range {n}) (range {n})))"),
        );
    }

    /// `min-by` / `max-by` over `n` number strings by string length.
    #[divan::bench(args = [100, 1_000])]
    fn min_max_by(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(def xs (map number->string (range {n}))) \
                 (do (min-by string-length xs) (max-by string-length xs))"
            ),
        );
    }
}
