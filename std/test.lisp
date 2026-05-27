;; std/test.lisp — a tiny test framework, written in mylisp itself.
;;
;;   (require 'test)            ; load the framework (embedded in the binary)
;;   (deftest math
;;     (is (= (+ 1 2) 3))
;;     (assert= (* 2 3) 6)
;;     (assert-error (/ 1 0)))
;;   (run-tests)
;;
;; `run-tests` prints a summary and raises an error if anything failed (so it
;; exits non-zero), which is how `cargo test` picks up failures.

;; Registry of (name . thunk), most-recently-defined first.
(def *tests* nil)
(def *passed* 0)
(def *failed* 0)
(def *current* nil)

;; (deftest name body...) — register a test. Its body runs when run-tests is called.
(defmacro deftest (name & body)
  `(set! *tests* (cons (cons (quote ~name) (fn () ~@body)) *tests*)))

;; (is expr) — assert expr is truthy.
(defn is (actual)
  (if actual
    (set! *passed* (+ *passed* 1))
    (do
      (set! *failed* (+ *failed* 1))
      (println "  FAIL" (str *current*) "— expected truthy, got" (pr-str actual)))))

;; (assert= actual expected) — assert structural equality.
(defn assert= (actual expected)
  (if (= actual expected)
    (set! *passed* (+ *passed* 1))
    (do
      (set! *failed* (+ *failed* 1))
      (println "  FAIL" (str *current*) "—" (pr-str actual) "≠" (pr-str expected)))))

;; (assert-error body...) — assert that evaluating body raises an error.
(defmacro assert-error (& body)
  `(if (try (do ~@body false) (catch e true))
     (set! *passed* (+ *passed* 1))
     (do
       (set! *failed* (+ *failed* 1))
       (println "  FAIL" (str *current*) "— expected an error, none raised"))))

;; Run the registered tests in definition order. Tail-recursive, so any number
;; of tests is fine.
(defn run-each (tests)
  (unless (empty? tests)
    (let (entry (first tests))
      (set! *current* (first entry))
      ((rest entry)))
    (run-each (rest tests))))

(defn run-tests ()
  (set! *passed* 0)
  (set! *failed* 0)
  (run-each (reverse *tests*))
  (println "")
  (println *passed* "passed," *failed* "failed  (" (count *tests*) "tests )")
  (if (> *failed* 0)
    (error (str *failed* " assertion(s) failed"))
    :ok))
