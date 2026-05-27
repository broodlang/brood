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

;; Registry of (name . thunk), most-recently-defined first.
(def *tests* nil)
(def *passed* 0)
(def *failed* 0)
(def *failures* nil)   ; detail strings for failed assertions, newest first
(def *current* nil)    ; name of the test currently running

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

;; Run the registered tests in definition order. Tail-recursive, so any number
;; of tests is fine. Prints `.` for a test that records no failure, else `F`.
(defn run-each (tests)
  (unless (empty? tests)
    (let (entry  (first tests)
          before *failed*)
      (set! *current* (first entry))
      ((rest entry))
      (print (if (> *failed* before) "F" ".")))
    (run-each (rest tests))))

;; Print the collected failure details, oldest first.
(defn report-failures ()
  (unless (empty? *failures*)
    (println "")
    (println "FAILURES:")
    (fold (fn (_ msg) (println (str "  " msg))) nil (reverse *failures*))))

(defn run-tests ()
  (set! *passed* 0)
  (set! *failed* 0)
  (set! *failures* nil)
  (let (start (now))
    (run-each (reverse *tests*))
    (let (elapsed (- (now) start))
      (println "")              ; end the progress line
      (report-failures)
      (println "")
      (println (count *tests*) "tests,"
               (+ *passed* *failed*) "assertions,"
               *failed* "failed"
               (str "(" elapsed " ms)"))
      (if (> *failed* 0)
        (error (str *failed* " assertion(s) failed"))
        :ok))))
