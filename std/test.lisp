;; std/test.lisp — a tiny test framework, written in Brood itself.
;;
;;   (require 'test)            ; load the framework (embedded in the binary)
;;   (deftest math
;;     (is (= (+ 1 2) 3))
;;     (assert= (* 2 3) 6)
;;     (assert-error (/ 1 0)))
;;   (run-tests)
;;
;; Running prints a progress line — one `.` per passing test, `F` per failing
;; one — then the detail of any failures, then a summary with the assertion
;; counts and how long the run took. `run-tests` raises an error if anything
;; failed (so the process exits non-zero), which is how `cargo test` notices.
;;
;; run-tests takes optional flags:
;;   (run-tests :trace)         ; one line per test — marker, name, time — instead of dots
;;   (run-tests :slow)          ; after the summary, list the slowest tests
;;   (run-tests :trace :slow)   ; both
;; Per-test times are recorded either way, so :slow works on its own.

;; Registry of (name . thunk), most-recently-defined first.
(def *tests* nil)
(def *passed* 0)
(def *failed* 0)
(def *failures* nil)   ; detail strings for failed assertions, newest first
(def *current* nil)    ; name of the test currently running
(def *timings* nil)    ; (name . ms) per test, newest first
(def *trace* false)    ; when true, run-each prints a line per test instead of dots

;; (deftest name body...) — register a test. Its body runs when run-tests is called.
(defmacro deftest (name & body)
  `(set! *tests* (cons (cons (quote ~name) (fn () ~@body)) *tests*)))

;; Assertions only tally a pass or record a failure — they print nothing, so the
;; progress dots stay on one tidy line. run-tests prints the details afterwards.
(defn pass! () (set! *passed* (+ *passed* 1)))
(defn fail! (msg)
  (set! *failed* (+ *failed* 1))
  (set! *failures* (cons (str *current* " — " msg) *failures*)))

;; (is expr) — assert expr is truthy.
(defn is (actual)
  (if actual
    (pass!)
    (fail! (str "expected truthy, got " (pr-str actual)))))

;; (assert= actual expected) — assert structural equality.
(defn assert= (actual expected)
  (if (= actual expected)
    (pass!)
    (fail! (str (pr-str actual) " ≠ " (pr-str expected)))))

;; (assert-error body...) — assert that evaluating body raises an error.
(defmacro assert-error (& body)
  `(if (try (do ~@body false) (catch e true))
     (pass!)
     (fail! "expected an error, none raised")))

;; --- small text/list helpers for the trace and slow-test output ---
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

;; One trace line: a pass/fail marker, the name, and the time it took.
(defn timing-line (name ms failed)
  (str "  " (if failed "F" ".") "  "
       (pad-right (str name) 24)
       (pad-left (str ms) 6) " ms"))

;; Run the registered tests in definition order. Tail-recursive, so any number
;; of tests is fine. Records each test's time, then either prints `.`/`F` or, in
;; trace mode, a per-test line.
(defn run-each (tests)
  (unless (empty? tests)
    (let (entry  (first tests)
          name   (first entry)
          before *failed*
          start  (now))
      (set! *current* name)
      ((rest entry))
      (let (ms     (- (now) start)
            failed (> *failed* before))
        (set! *timings* (cons (cons name ms) *timings*))
        (if *trace*
          (println (timing-line name ms failed))
          (print (if failed "F" ".")))))
    (run-each (rest tests))))

;; Print the collected failure details, oldest first.
(defn report-failures ()
  (unless (empty? *failures*)
    (println "")
    (println "FAILURES:")
    (fold (fn (_ msg) (println (str "  " msg))) nil (reverse *failures*))))

;; Print the slowest tests, slowest first (at most 5).
(defn report-slow ()
  (println "")
  (println "Slowest tests:")
  (fold (fn (_ entry)
          (println (str "  " (pad-right (str (first entry)) 24)
                        (pad-left (str (rest entry)) 6) " ms")))
        nil
        (take 5 (sort-by-ms *timings*))))

;; Integer division, and a "N.N MB" formatter for the memory line. `rem` is the
;; integer-remainder primitive; `(- a (rem a b))` is divisible by b, so here `/`
;; lands on an integer rather than a float.
(defn quot (a b) (/ (- a (rem a b)) b))
(defn mb-str (bytes)
  (let (tenths (quot (+ (* bytes 10) 524288) 1048576)   ; round to nearest 0.1 MB
        whole  (quot tenths 10)
        frac   (rem tenths 10))
    (str whole "." frac " MB")))

(defn run-tests (& opts)
  (set! *passed* 0)
  (set! *failed* 0)
  (set! *failures* nil)
  (set! *timings* nil)
  (set! *trace* (opt? :trace opts))
  (let (start (now))
    (run-each (reverse *tests*))
    (let (elapsed (- (now) start))
      (println "")              ; end the progress line
      (report-failures)
      (when (opt? :slow opts) (report-slow))
      (println "")
      (println (count *tests*) "tests,"
               (+ *passed* *failed*) "assertions,"
               *failed* "failed")
      (println (str "  (" elapsed " ms, peak " (mb-str (mem-peak)) ")"))
      (if (> *failed* 0)
        (error (str *failed* " assertion(s) failed"))
        :ok))))
