;; mylisp test suite — written in mylisp itself.
;;   ./bin/cli tests/suite.lisp     (prints a summary; exits non-zero on failure)
;; It is also run by `cargo test` (see crates/lisp/tests/suite.rs).

(def *passed* 0)
(def *failed* 0)

(defn check (name actual expected)
  (if (= actual expected)
    (set! *passed* (+ *passed* 1))
    (do
      (set! *failed* (+ *failed* 1))
      (println "FAIL:" name "— expected" (pr-str expected) "got" (pr-str actual)))))

;; ---- arithmetic ----
(check "add"        (+ 1 2 3)         6)
(check "sub"        (- 10 3 2)        5)
(check "neg"        (- 5)             -5)
(check "mul"        (* 2 3 4)         24)
(check "div-int"    (/ 12 3)          4)
(check "div-float"  (/ 7 2)           3.5)
(check "mod"        (mod 10 3)        1)
(check "nested"     (+ 1 (* 2 3))     7)

;; ---- comparison / equality ----
(check "lt"         (< 1 2 3)         true)
(check "lt-false"   (< 1 3 2)         false)
(check "eq"         (= 2 2)           true)
(check "not="       (not= 1 2)        true)
(check "eq-list"    (= (list 1 2) (list 1 2)) true)

;; ---- lists ----
(check "cons"       (cons 0 (list 1 2))      (list 0 1 2))
(check "first"      (first (list 1 2 3))     1)
(check "rest"       (rest (list 1 2 3))      (list 2 3))
(check "count"      (count (list 1 2 3 4))   4)
(check "reverse"    (reverse (list 1 2 3))   (list 3 2 1))
(check "append"     (append (list 1 2) (list 3 4)) (list 1 2 3 4))
(check "nth-list"   (nth (list 10 20 30) 1)  20)

;; ---- higher order ----
(check "map"        (map inc (list 1 2 3))               (list 2 3 4))
(check "filter"     (filter positive? (list -1 2 -3 4))  (list 2 4))
(check "reduce"     (reduce + 0 (list 1 2 3 4))          10)
(check "apply"      (apply + (list 1 2 3))               6)

;; ---- vectors (data) ----
(check "vector-eval" [1 (+ 1 1) 3]   [1 2 3])
(check "nth-vec"     (nth [10 20 30] 2) 30)
(check "vec-count"   (count [1 2 3])  3)

;; ---- strings ----
(check "str"        (str "a" "b" 3)  "ab3")
(check "str-len"    (count "hello")  5)

;; ---- predicates ----
(check "nil?"       (nil? nil)       true)
(check "pair?"      (pair? (list 1)) true)
(check "number?"    (number? 3.5)    true)
(check "vector?"    (vector? [1])    true)

;; ---- conditionals / let / closures ----
(check "if"      (if (< 1 2) :y :n)                  :y)
(check "cond"    (cond false :a true :b else :c)     :b)
(check "when"    (when true 1 2 3)                   3)
(check "let"     (let (a 1 b (+ a 1)) (+ a b))       3)
(check "closure" (let (adder (fn (a) (fn (b) (+ a b)))) ((adder 3) 4)) 7)

;; ---- parameter forms ----
(check "optional-default" ((fn (a &optional (b 10)) (+ a b)) 5)   15)
(check "optional-given"   ((fn (a &optional (b 10)) (+ a b)) 5 1) 6)
(check "rest-args"        ((fn (& xs) xs) 1 2 3)                   (list 1 2 3))

;; ---- defn / macros / threading ----
(defn sq (x) (* x x))
(check "defn" (sq 6) 36)

(defmacro my-when (c & body) `(if ~c (do ~@body) nil))
(check "macro-true"  (my-when true 1 2 3)  3)
(check "macro-false" (my-when false 1 2 3) nil)
(check "quasiquote"  `(1 ~(+ 1 1) ~@(list 3 4) 5) (list 1 2 3 4 5))

(check "thread-first" (-> 5 (- 1) (* 2))         8)
(check "thread-last"  (->> (list 1 2 3) (map inc)) (list 2 3 4))

;; ---- error handling ----
(check "catch-throw"   (try (throw 42) (catch e e))            42)
(check "catch-builtin" (try (/ 1 0) (catch e (string? e)))     true)
(check "no-throw"      (try (+ 1 2) (catch e :nope))           3)

;; ---- tail recursion (must not overflow) ----
(defn sum-to (n acc) (if (= n 0) acc (sum-to (- n 1) (+ acc n))))
(check "tail-calls" (sum-to 100000 0) 5000050000)

;; ---- summary ----
(println "")
(println *passed* "passed," *failed* "failed")
(if (> *failed* 0)
  (error (str *failed* " test(s) failed"))
  (println "mylisp suite: all passed"))
