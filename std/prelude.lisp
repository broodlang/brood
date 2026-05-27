;; mylisp prelude / core library
;; ------------------------------
;; Most of the language lives here, in mylisp itself. Rust provides only a small
;; primitive kernel (see crates/lisp/src/builtins.rs — the `%`-prefixed ops and
;; a few constructors/predicates); everything below is ordinary mylisp.
;;
;; Style note: code is made of lists, so parameter lists are written as lists —
;; (defn f (x y) ...). Vectors [ ] are a *data* type (O(1) indexing); they are
;; still accepted in parameter/binding positions, but lists are idiomatic.
;;
;; The very first definition is `defn`, a macro. Everything after it is written
;; with `defn`, so the library is defined exactly the way you would define your
;; own functions — including `+`, which is "just a function".

;; `defn` itself: (defn name (params) body...) => (def name (fn (params) body...))
(defmacro defn (name params & body)
  `(def ~name (fn ~params ~@body)))

;; ---- logic ----
(defn not (x) (if x false true))

;; ---- folding (the workhorse; tail-recursive, so it is stack-safe) ----
(defn fold (f acc coll)
  (if (empty? coll)
    acc
    (fold f (f acc (first coll)) (rest coll))))

(defn reverse (coll)
  (fold (fn (acc x) (cons x acc)) nil coll))

;; reduce supports both (reduce f init coll) and (reduce f coll).
(defn reduce (f x & more)
  (if (empty? more)
    (fold f (first x) (rest x))
    (fold f x (first more))))

;; ---- arithmetic (variadic, built on the 2-arg primitives) ----
(defn + (& xs) (reduce %add 0 xs))
(defn * (& xs) (reduce %mul 1 xs))
(defn - (x & xs) (if (empty? xs) (%sub 0 x) (reduce %sub x xs)))
(defn / (x & xs) (if (empty? xs) (%div 1 x) (reduce %div x xs)))

(defn inc (n) (%add n 1))
(defn dec (n) (%sub n 1))

;; ---- comparison & equality (variadic chains over %lt / %eq) ----
;; chain? checks a predicate holds for every adjacent pair: (chain? < (1 2 3)).
(defn chain? (pred xs)
  (if (empty? xs)
    true
    (if (empty? (rest xs))
      true
      (if (pred (first xs) (first (rest xs)))
        (chain? pred (rest xs))
        false))))

(defn <  (& xs) (chain? (fn (a b) (%lt a b)) xs))
(defn >  (& xs) (chain? (fn (a b) (%lt b a)) xs))
(defn <= (& xs) (chain? (fn (a b) (not (%lt b a))) xs))
(defn >= (& xs) (chain? (fn (a b) (not (%lt a b))) xs))
(defn =  (& xs) (chain? (fn (a b) (%eq a b)) xs))
(defn not= (& xs) (not (apply = xs)))

;; ---- derived predicates ----
(defn number? (x) (or (int? x) (float? x)))
(defn list?   (x) (or (nil? x) (pair? x)))

;; ---- list aliases & accessors ----
(def car first)
(def cdr rest)
(defn list (& xs) xs)
(defn second (coll) (first (rest coll)))
(defn third  (coll) (first (rest (rest coll))))

;; ---- sequence operations ----
(defn map (f coll)
  (reverse (fold (fn (acc x) (cons (f x) acc)) nil coll)))

(defn filter (pred coll)
  (reverse (fold (fn (acc x) (if (pred x) (cons x acc) acc)) nil coll)))

(defn append-two (a b)
  (fold (fn (acc x) (cons x acc)) b (reverse a)))
(defn append (& lists) (fold append-two nil lists))

(defn count (coll)
  (if (string? coll)
    (string-length coll)
    (fold (fn (acc x) (inc acc)) 0 coll)))
(def length count)

(defn nth-list (coll i d)
  (cond
    (empty? coll) d
    (= i 0)       (first coll)
    else          (nth-list (rest coll) (- i 1) d)))

(defn nth (coll i & default)
  (if (vector? coll)
    (if (and (>= i 0) (< i (vector-length coll)))
      (vector-ref coll i)
      (first default))
    (nth-list coll i (first default))))

;; ---- numeric helpers ----
(defn identity  (x) x)
(defn zero?     (n) (= n 0))
(defn positive? (n) (> n 0))
(defn negative? (n) (< n 0))
(defn abs (n) (if (< n 0) (- n) n))
(defn max (a b) (if (> a b) a b))
(defn min (a b) (if (< a b) a b))
(defn sum     (xs) (reduce + 0 xs))
(defn product (xs) (reduce * 1 xs))

;; ---- threading macros (demonstrate that macros compute at expansion time) ----
;; (-> x (f a) (g b))  =>  (g (f x a) b)   ; thread as FIRST argument
;; (->> x (f a) (g b)) =>  (g b (f a x))   ; thread as LAST argument
(defn thread-first-step (acc form)
  (if (pair? form)
    (cons (first form) (cons acc (rest form)))
    (list form acc)))
(defmacro -> (x & forms)
  (reduce thread-first-step x forms))

(defn thread-last-step (acc form)
  (if (pair? form)
    (append form (list acc))
    (list form acc)))
(defmacro ->> (x & forms)
  (reduce thread-last-step x forms))
