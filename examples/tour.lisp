;; mylisp feature tour
;; -------------------
;; Run it:   ./bin/cli examples/tour.lisp
;; Or play:  ./bin/cli        (starts the REPL)

;; Arithmetic — +, -, *, /, etc. are defined in mylisp on a tiny Rust kernel.
(println "1 + 2 * 3   =" (+ 1 (* 2 3)))
(println "7 / 2       =" (/ 7 2))
(println "10 mod 3    =" (mod 10 3))

;; defn, with optional args (default values)
(defn greet (name &optional (greeting "hello"))
  (str greeting ", " name))
(println (greet "world"))
(println (greet "world" "hi"))

;; Recursion is the loop — proper tail calls, so this doesn't grow the stack.
(defn sum-to (n acc) (if (= n 0) acc (sum-to (- n 1) (+ acc n))))
(println "sum 1..100000 =" (sum-to 100000 0))

;; Higher-order functions over lists.
(println "squares:" (map (fn (x) (* x x)) (list 1 2 3 4 5)))
(println "evens:  " (filter (fn (x) (= 0 (mod x 2))) (list 1 2 3 4 5 6)))
(println "total:  " (reduce + 0 (list 1 2 3 4 5)))

;; Vectors are a data type with O(1) indexing (code is lists, data can be vectors).
(def v [10 20 30])
(println "v[1] =" (nth v 1) " count =" (count v))

;; Macros — define your own control form.
(defmacro unless2 (c & body) `(if ~c nil (do ~@body)))
(println (unless2 false "the body ran"))

;; Threading macro.
(println "(-> 5 (+ 3) (* 2)) =" (-> 5 (+ 3) (* 2)))

;; Error handling.
(println (try (/ 1 0) (catch e (str "caught: " e))))
(println (try (throw :boom) (catch e (str "caught: " e))))

(println "--- tour complete ---")
