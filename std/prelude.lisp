;; mylisp prelude
;; ----------------
;; A few helpers defined in the language itself rather than in Rust. Keeping
;; these here (instead of as builtins) is the whole point: the more of mylisp
;; that is written in mylisp, the more of it you can redefine while it runs.
;;
;; v0.1 has no macros yet, so everything here is a plain `def` of a `fn`.

(def inc (fn [n] (+ n 1)))
(def dec (fn [n] (- n 1)))

(def identity (fn [x] x))
(def second (fn [coll] (first (rest coll))))
(def third (fn [coll] (first (rest (rest coll)))))

(def zero?     (fn [n] (= n 0)))
(def positive? (fn [n] (> n 0)))
(def negative? (fn [n] (< n 0)))

;; Numeric helpers.
(def abs (fn [n] (if (< n 0) (- n) n)))
(def max (fn [a b] (if (> a b) a b)))
(def min (fn [a b] (if (< a b) a b)))

;; Sum / product of a list, expressed with reduce.
(def sum     (fn [xs] (reduce + 0 xs)))
(def product (fn [xs] (reduce * 1 xs)))
