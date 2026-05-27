;; std/test.lisp — a tiny test framework, written in Brood itself.
;;
;;   (require 'test)
;;   (describe "math"
;;     (test "adds"       (assert= (+ 1 2) 3))
;;     (test "multiplies" (assert= (* 2 3) 6) (assert-error (/ 1 0))))
;;   (run-tests)
;;
;; ExUnit / `mix test`-style: `describe` groups related cases, `test` names a
;; case with a string. `deftest` (one test named by a symbol, no group) is kept
;; for convenience and expands to `test`. See docs/testing.md for the full guide.
;;
;; ─────────────────────────────────────────────────────────────────────────────
;; EXECUTION MODEL — parallel by default, with opt-in serialisation
;; ─────────────────────────────────────────────────────────────────────────────
;; Every test runs concurrently, each in its own process (spawn/send/receive),
;; UNLESS its group opts out. Two opt-outs, given as a keyword right after the
;; group/name:
;;
;;   (describe "fast stuff" ...)            ; default: each test runs in parallel
;;   (describe "uses a shared file" :serial ...)   ; its tests run one-at-a-time,
;;                                          ;   in a single worker, but still
;;                                          ;   alongside *other* groups
;;   (describe "mutates a global" :isolated ...)   ; runs ALONE, against a PRIVATE
;;                                          ;   copy of the globals: no other test
;;                                          ;   runs at the same time, and its
;;                                          ;   def/set! roll back when it finishes
;;   (test "touches global state" :isolated ...)   ; a lone isolated test
;;
;; Why this matters here: a runtime's processes SHARE one global table (see
;; docs/shared-code.md), so two parallel tests that both redefine the same global
;; would race. Mark such a group `:serial` (serialise within the group) or
;; `:isolated` (run alone AND with a rolled-back private copy of the globals, so a
;; test's definitions can't leak to any other test). Tests that only read the
;; prelude and their own locals are safe to run in parallel — the default.
;;
;; Phases: the :isolated units run FIRST — one at a time on the runner itself,
;; each under `%isolate` (a private copy of the globals) — so each sees the clean
;; post-load baseline and nothing it defs survives. THEN all :parallel and :serial
;; units are spawned and run together.
;;
;; ─────────────────────────────────────────────────────────────────────────────
;; SHARE-SAFE TALLYING
;; ─────────────────────────────────────────────────────────────────────────────
;; Because processes share the global table, tests must NOT tally into shared
;; mutable globals (concurrent tests would clobber one another). Instead each test
;; collects its own failures into a PROCESS-LOCAL accumulator — `*fails*`, a `let`
;; binding the `test` macro establishes — and yields them as a value; the runner
;; aggregates results from returns / messages into its own local state. No shared
;; counters, no races. (Assertions are macros that push onto `*fails*`, so use
;; them lexically inside a test body, not from unrelated top-level helpers.)
;;
;; run-tests prints progress (a `.`/`F` per test, or a ✓/✗ line per test with
;; :trace), then any failures, then a summary with timing and memory. It raises an
;; error if anything failed (non-zero exit — how `cargo test` notices). Flags:
;;   (run-tests :trace)   ; a per-test line as each finishes, instead of dots
;;   (run-tests :slow)    ; after the summary, list the slowest tests

;; --- registry ----------------------------------------------------------------
;; Built once, at load time, on the main process — so touching these globals here
;; is single-threaded and safe; workers never touch them.
;;
;; A *unit* is the thing scheduled together: (mode group tests), where mode is
;; :parallel | :serial | :isolated and tests is a list of (name thunk). A thunk
;; runs one test body and returns its list of failure-message strings (empty =
;; passed). A default `describe` emits one :parallel unit PER test (max
;; parallelism); a :serial/:isolated `describe` emits ONE unit holding all its
;; tests (so they run in sequence, in a single process).
(def *units* nil)         ; all registered units, newest first
(def *collecting* false)  ; true while a describe body is being registered
(def *collected* nil)     ; (name thunk) pairs gathered for the current describe

(defn add-unit! (mode group tests)
  (set! *units* (cons (list mode group tests) *units*)))

;; A test registers itself: collected into the enclosing describe if there is
;; one, otherwise emitted as its own standalone unit (:isolated if flagged).
(defn register-test! (name thunk isolated?)
  (if *collecting*
    (set! *collected* (cons (list name thunk) *collected*))
    (add-unit! (if isolated? :isolated :parallel) "" (list (list name thunk)))))

;; Turn a finished describe's collected tests into units according to its mode:
;; default spreads them across one :parallel unit each; :serial/:isolated keep
;; them together in a single unit.
(defn emit-describe! (mode group tests)
  (if (= mode :parallel)
    (fold (fn (_ t) (add-unit! :parallel group (list t))) nil tests)
    (add-unit! mode group tests)))

;; (describe "group" [:serial | :isolated] body...) — register a group of tests.
(defmacro describe (group & forms)
  (let (mode (cond (= (first forms) :isolated) :isolated
                   (= (first forms) :serial)   :serial
                   else                        :parallel)
        body (if (= mode :parallel) forms (rest forms)))
    `(do
       (set! *collecting* true)
       (set! *collected* nil)
       (do ~@body)                          ; the (test ...) forms collect themselves
       (set! *collecting* false)
       (emit-describe! ~mode ~group (reverse *collected*))
       nil)))

;; (test "name" [:isolated] body...) — register one test. Its body runs later,
;; inside a process-local *fails* accumulator the assertions push onto. An
;; uncaught error becomes a failure (so a crashing test can't hang the runner: a
;; dead worker never sends, and `receive` would block forever).
(defmacro test (name & forms)
  (let (iso  (= (first forms) :isolated)
        body (if iso (rest forms) forms))
    `(register-test! ~name
       (fn ()
         (let (*fails* nil)
           (try (do ~@body)
                (catch e (set! *fails* (cons (str "uncaught error: " e) *fails*))))
           (reverse *fails*)))
       ~iso)))

;; (deftest name body...) — a single test named by a symbol, no group.
(defmacro deftest (name & body)
  `(test (str (quote ~name)) ~@body))

;; --- assertions --------------------------------------------------------------
;; Macros, not functions: on failure each pushes a message onto the enclosing
;; test's *fails*; on success they do nothing. They evaluate their operands once,
;; and they DON'T stop the test — every assertion in a body runs, so one test can
;; report several failures.
;; Each assertion reports the FAILING SOURCE EXPRESSION (quoted at macro time) and
;; the actual value — so a failure line is self-identifying: you (or an LLM reading
;; the captured run) see exactly which expression failed and what it produced,
;; without opening the file or guessing among look-alike assertions.
(defmacro is (expr)
  (let (v (gensym "v"))
    `(let (~v ~expr)
       (unless ~v
         (set! *fails* (cons (str (pr-str (quote ~expr)) " is " (pr-str ~v)) *fails*))))))

;; (refute expr) — the negation of `is`: fail if expr is truthy. Reports the
;; truthy value it got where it wanted nil/false.
(defmacro refute (expr)
  (let (v (gensym "v"))
    `(let (~v ~expr)
       (when ~v
         (set! *fails* (cons (str (pr-str (quote ~expr)) " is " (pr-str ~v) " — expected falsy") *fails*))))))

(defmacro assert= (actual expected)
  (let (a (gensym "a") b (gensym "b"))
    `(let (~a ~actual ~b ~expected)
       (unless (= ~a ~b)
         (set! *fails* (cons (str (pr-str (quote ~actual)) " => " (pr-str ~a)
                                  ", expected " (pr-str ~b)) *fails*))))))

(defmacro assert-error (& body)
  ;; Quote the body for the message: the lone form if there's one, else (do …).
  (let (form (if (= (count body) 1) (first body) (cons 'do body)))
    `(unless (try (do ~@body false) (catch e true))
       (set! *fails* (cons (str "expected " (pr-str (quote ~form)) " to raise, but none did") *fails*)))))

;; (error-of body...) — evaluate body and yield the error it raised, so you can
;; assert on the *output*: a built-in error comes back as its message string
;; (e.g. "type error: expected a number"), a `(throw v)` as the value v itself.
;; Returns nil when body completes without raising. Pair it with assert= to pin
;; exact failure output:  (assert= (error-of (/ 1 0)) "runtime error: division by zero")
(defmacro error-of (& body)
  `(try (do ~@body nil) (catch e e)))

;; --- small text/list helpers for the trace and slow-test output --------------
(defn opt? (flag opts)                    ; is flag present in the run-tests args?
  (cond
    (empty? opts)         false
    (= flag (first opts)) true
    else                  (opt? flag (rest opts))))

(defn spaces (n) (if (<= n 0) "" (str " " (spaces (- n 1)))))
(defn pad-right (s w) (str s (spaces (- w (count s)))))
(defn pad-left  (s w) (str (spaces (- w (count s))) s))

(defn take (n xs)                         ; first n elements (fewer if xs is shorter)
  (if (<= n 0)
    nil
    (if (empty? xs)
      nil
      (cons (first xs) (take (- n 1) (rest xs))))))

;; Insertion sort of (name . ms) pairs, slowest first. O(n²), but there are only
;; ever a handful of tests, so simplicity wins.
(defn insert-by-ms (x sorted)
  (cond
    (empty? sorted)                     (list x)
    (>= (rest x) (rest (first sorted))) (cons x sorted)
    else                                (cons (first sorted) (insert-by-ms x (rest sorted)))))
(defn sort-by-ms (xs) (fold (fn (acc x) (insert-by-ms x acc)) nil xs))

;; Integer division, and a "N.N MB" formatter for the memory line.
(defn quot (a b) (/ (- a (rem a b)) b))
(defn mb-str (bytes)
  (let (tenths (quot (+ (* bytes 10) 524288) 1048576)   ; round to nearest 0.1 MB
        whole  (quot tenths 10)
        frac   (rem tenths 10))
    (str whole "." frac " MB")))

;; ANSI helpers (the reader's `\e` is ESC). Markers/colours sit *outside* the
;; padded name, so column alignment still counts only visible text. Colour is
;; emitted ONLY when stdout is a real terminal — when the run is captured (a
;; pipe, `cargo test`, an LLM or CI reading it), `ansi` returns the plain string,
;; so the output is clean text with no escape-code noise.
(defn ansi (code s) (if (stdout-tty?) (str "\e[" code "m" s "\e[0m") s))
(defn green  (s) (ansi "32" s))
(defn red    (s) (ansi "31" s))
(defn dim    (s) (ansi "2"  s))

;; --- results -----------------------------------------------------------------
;; A result is (group name fails ms): `fails` is the list of failure messages for
;; that test (empty = passed).
(defn r-group (r) (first r))
(defn r-name  (r) (second r))
(defn r-fails (r) (third r))
(defn r-ms    (r) (nth r 3))
(defn r-failed? (r) (not (empty? (r-fails r))))
(defn label (group name)                  ; "group: name", or just name if ungrouped
  (if (= group "") (str name) (str group ": " name)))
(defn full-name (r) (label (r-group r) (r-name r)))

;; Run one (name thunk) and produce its result (group name fails ms). The thunk's
;; failures land in its own process-local *fails*, so running these concurrently
;; is safe.
(defn run-test (group t)
  (let (start (now)
        fails ((second t))
        ms    (- (now) start))
    (list group (first t) fails ms)))

;; Like run-test, but evaluate the body with a private copy of the global
;; bindings (`%isolate`): an :isolated test's def/set! is rolled back when it
;; finishes, so it can't leak to any other test. Used only in the isolated phase.
(defn run-test-fresh (group t)
  (let (start (now)
        fails (%isolate (second t))
        ms    (- (now) start))
    (list group (first t) fails ms)))

;; Run every test in a unit, in order, returning the list of results. This is
;; what a worker does (for a :parallel/:serial unit) and what the runner does
;; directly for an :isolated unit.
(defn run-unit (unit)
  (let (group (second unit))
    (map (fn (t) (run-test group t)) (third unit))))

;; One trace line: a pass/fail marker, the group-qualified name, and the time.
(defn timing-line (r)
  (str "  " (if (r-failed? r) (red "✗") (green "✓")) "  "
       (pad-right (full-name r) 28)
       (pad-left (str (r-ms r)) 6) " ms"))

;; Print a result as it becomes known: a per-test line in :trace, else a dot/F.
(defn render-result (trace? r)
  (if trace?
    (println (timing-line r))
    (print (if (r-failed? r) "F" "."))))

;; --- the two phases ----------------------------------------------------------
;; Parallel phase: spawn one worker per non-isolated unit (each worker captures
;; its unit and reports the unit's results back as one message), then collect.
(defn spawn-units (units me)
  (fold (fn (_ unit)
          (spawn (fn (parent) (send parent (run-unit unit))) me))
        nil units))

(defn collect-units (trace? remaining acc)   ; remaining = messages still to receive
  (if (= remaining 0)
    acc
    (let (results (receive))                  ; one unit's worth of results
      (fold (fn (_ r) (render-result trace? r)) nil results)
      (collect-units trace? (- remaining 1) (append results acc)))))

;; Isolated phase: run each isolated unit on the runner itself, one at a time, so
;; nothing else is executing — and each test under `%isolate`, so its global defs
;; are rolled back and can't leak to another test.
(defn run-isolated (trace? units acc)
  (if (empty? units)
    acc
    (let (unit    (first units)
          group   (second unit)
          results (map (fn (t) (run-test-fresh group t)) (third unit)))
      (fold (fn (_ r) (render-result trace? r)) nil results)
      (run-isolated trace? (rest units) (append results acc)))))

;; --- reporting ---------------------------------------------------------------
(defn report-failures (results)
  (let (failed (filter r-failed? results))
    (unless (empty? failed)
      (println "")
      (println "FAILURES:")
      (fold (fn (_ r)
              (fold (fn (_ msg) (println (str "  " (full-name r) " — " msg)))
                    nil (r-fails r)))
            nil failed))))

(defn report-slow (results)
  (println "")
  (println "Slowest tests:")
  (let (timings (map (fn (r) (cons (full-name r) (r-ms r))) results))
    (fold (fn (_ entry)
            (println (str "  " (pad-right (str (first entry)) 28)
                          (pad-left (str (rest entry)) 6) " ms")))
          nil
          (take 5 (sort-by-ms timings)))))

(defn count-failed (results)              ; tests with at least one failed assertion
  (fold (fn (acc r) (if (r-failed? r) (+ acc 1) acc)) 0 results))
(defn count-failures (results)            ; total failed assertions across tests
  (fold (fn (acc r) (+ acc (count (r-fails r)))) 0 results))
(defn sum-ms (results)                    ; summed runtime (ms) of the given tests
  (fold (fn (acc r) (+ acc (r-ms r))) 0 results))

;; --- entry point -------------------------------------------------------------
(defn run-tests (& opts)
  (let (trace?  (opt? :trace opts)
        units   (reverse *units*)
        par     (filter (fn (u) (not= (first u) :isolated)) units)  ; parallel + serial
        iso     (filter (fn (u) (= (first u) :isolated)) units)
        spawn0  (spawn-count)
        start   (now)
        me      (self))
    ;; Isolated phase first: each isolated test runs alone, against a private copy
    ;; of the globals, so it sees the clean post-load baseline (none of the
    ;; parallel/serial defs below) and its own defs roll back. Then launch the
    ;; parallel/serial workers and collect them.
    (let (iso-results (run-isolated trace? iso nil)
          _           (spawn-units par me)
          par-results (collect-units trace? (count par) nil)
          results     (append iso-results par-results)
          elapsed     (- (now) start)
          spawned     (- (spawn-count) spawn0)
          workers     (count par)               ; one worker per parallel/serial unit
          nested      (- spawned workers)        ; extra processes the tests spawned
          total       (count results)
          failed      (count-failed results)
          fails       (count-failures results)
          ;; Total test runtime broken down by execution variation. It's the SUM
          ;; of each test's own duration, so the parallel/serial figure exceeds
          ;; its share of the wall clock (those tests overlap on worker threads);
          ;; the isolated figure is wall-clock-like (they run one at a time).
          iso-time    (sum-ms iso-results)
          par-time    (sum-ms par-results)
          test-time   (+ iso-time par-time))
      (println "")              ; end the progress line
      (report-failures results)
      (when (opt? :slow opts) (report-slow results))
      (println "")
      (println total "tests," (- total failed) "passed," failed "failed"
               (str "(" fails " failed assertions, " (count iso) " isolated)"))
      (println (str "  test runtime: " test-time " ms total — "
                    "parallel/serial " par-time " ms, isolated " iso-time " ms"))
      (println (str "  (" elapsed " ms wall, peak " (mb-str (mem-peak)) ")"))
      ;; Processes are cheap green coroutines (step 4b), multiplexed onto a fixed
      ;; pool of worker OS threads (≈ nproc) — NOT one thread each. `peak` is the
      ;; most that ran simultaneously (bounded by the pool), so it shows the real
      ;; parallelism reached.
      (println (str "  " (+ spawned 1) " processes (1 runner + " workers
                    " unit workers + " nested " nested) on " (worker-threads)
                    " worker threads, peak " (peak-threads) " running at once"))
      (if (> failed 0)
        (error (str failed " test(s) failed"))
        :ok))))
