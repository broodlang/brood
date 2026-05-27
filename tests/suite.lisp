;; Brood test suite — written in Brood, using the native test library.
;;   ./bin/cli tests/suite.lisp
;; (also run by `cargo test` via crates/lisp/tests/suite.rs)

(require 'test)

(deftest arithmetic
  (assert= (+ 1 2 3)     6)
  (assert= (- 10 3 2)    5)
  (assert= (- 5)         -5)
  (assert= (* 2 3 4)     24)
  (assert= (/ 12 3)      4)
  (assert= (/ 7 2)       3.5)
  (assert= (mod 10 3)    1)
  (assert= (+ 1 (* 2 3)) 7))

(deftest comparison-and-equality
  (is (< 1 2 3))
  (is (not (< 1 3 2)))
  (is (= 2 2))
  (is (not= 1 2))
  (assert= (list 1 2) (list 1 2)))

(deftest lists
  (assert= (cons 0 (list 1 2))             (list 0 1 2))
  (assert= (first (list 1 2 3))            1)
  (assert= (rest (list 1 2 3))             (list 2 3))
  (assert= (count (list 1 2 3 4))          4)
  (assert= (reverse (list 1 2 3))          (list 3 2 1))
  (assert= (append (list 1 2) (list 3 4))  (list 1 2 3 4))
  (assert= (nth (list 10 20 30) 1)         20))

(deftest higher-order
  (assert= (map inc (list 1 2 3))               (list 2 3 4))
  (assert= (filter positive? (list -1 2 -3 4))  (list 2 4))
  (assert= (reduce + 0 (list 1 2 3 4))          10)
  (assert= (apply + (list 1 2 3))               6))

(deftest vectors
  (assert= [1 (+ 1 1) 3]   [1 2 3])
  (assert= (nth [10 20 30] 2) 30)
  (assert= (count [1 2 3]) 3))

(deftest strings
  (assert= (str "a" "b" 3) "ab3")
  (assert= (count "hello") 5))

(deftest predicates
  (is (nil? nil))
  (is (pair? (list 1)))
  (is (number? 3.5))
  (is (vector? [1])))

(deftest control-and-binding
  (assert= (if (< 1 2) :y :n)              :y)
  (assert= (cond false :a true :b else :c) :b)
  (assert= (when true 1 2 3)               3)
  (assert= (let (a 1 b (+ a 1)) (+ a b))   3)
  (assert= (let (adder (fn (a) (fn (b) (+ a b)))) ((adder 3) 4)) 7))

(deftest parameter-forms
  (assert= ((fn (a &optional (b 10)) (+ a b)) 5)   15)
  (assert= ((fn (a &optional (b 10)) (+ a b)) 5 1) 6)
  (assert= ((fn (& xs) xs) 1 2 3)                  (list 1 2 3)))

(deftest defn-and-macros
  (defn sq (x) (* x x))
  (assert= (sq 6) 36)
  (defmacro my-when (c & body) `(if ~c (do ~@body) nil))
  (assert= (my-when true 1 2 3)  3)
  (assert= (my-when false 1 2 3) nil)
  (assert= `(1 ~(+ 1 1) ~@(list 3 4) 5) (list 1 2 3 4 5)))

(deftest threading
  (assert= (-> 5 (- 1) (* 2))          8)
  (assert= (->> (list 1 2 3) (map inc)) (list 2 3 4)))

(deftest error-handling
  (assert= (try (throw 42) (catch e e))         42)
  (is      (try (/ 1 0) (catch e (string? e))))
  (assert= (try (+ 1 2) (catch e :nope))        3)
  (assert-error (/ 1 0))
  (assert-error (throw :boom)))

(deftest tail-calls
  (defn sum-to (n acc) (if (= n 0) acc (sum-to (- n 1) (+ acc n))))
  (assert= (sum-to 100000 0) 5000050000))

(deftest processes
  ;; spawn a worker, send it a number, get back double — across OS threads.
  (defn echo-double (parent) (send parent (* (receive) 2)))
  (let (w (spawn echo-double (self)))
    (send w 21)
    (assert= (receive) 42)))

(run-tests)
