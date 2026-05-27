;; tests/suite-failures.lisp — a deliberately-failing suite, for *looking at*.
;;
;;   ./bin/cli tests/suite-failures.lisp
;;
;; Run it by hand to see how the framework renders a mix of passing and failing
;; tests: the per-test ✓/✗ markers, then the FAILURES section with one line per
;; failed assertion. Every failure below is on purpose, so this file is NOT wired
;; into `cargo test` — it would fail the build by design.
;;
;; Tallying is share-safe (each test collects its own failures locally and the
;; runner aggregates from results — see std/test.lisp), so this renders correctly
;; in either mode: try `(run-tests :parallel :trace)` too — failures stay
;; attributed to the right test and counts don't double.
;;
;; run-tests raises "N test(s) failed" at the very end (that non-zero exit is how
;; cargo test notices real failures). We catch that last error so the demo
;; finishes by showing its report rather than dumping a backtrace.

(require 'test)

;; --- tests that pass, so the report shows successes alongside failures ------
(describe "passing examples"
  (test "arithmetic" (assert= (+ 1 2) 3) (assert= (* 2 3) 6) (is (< 1 2 3)))
  (test "lists"      (assert= (reverse (list 1 2 3)) (list 3 2 1)) (is (empty? nil))))

;; --- tests that fail, one per kind of failure the framework reports ---------
(describe "failing examples"
  ;; assert= prints both sides with the ≠ marker.
  (test "assert= shows both sides"
    (assert= (+ 2 2) 5)
    (assert= (str "a" "b") "abc"))
  ;; is reports the falsy value it got where it wanted something truthy.
  (test "is reports the falsy value"
    (is (= 1 2))
    (is (empty? (list 1))))
  ;; assert-error fails when the body does NOT raise.
  (test "assert-error needs a raise"
    (assert-error (+ 1 2)))
  ;; A test collects every failed assertion (it doesn't stop at the first), and
  ;; only the failing ones are reported.
  (test "mixed passes and failures"
    (assert= (count "hello") 5)   ; pass
    (is (number? "nope"))         ; fail
    (assert= :ok :ok)))           ; pass

;; Show the report, then swallow the final "N test(s) failed" so the demo exits
;; cleanly instead of aborting with a backtrace.
(try (run-tests :trace)
  (catch e (println (str "\n(run-tests signalled: " e " — expected for this demo)"))))
