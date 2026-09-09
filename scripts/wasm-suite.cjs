// The wasm32 behavioural cases. Driven by scripts/wasm-suite.sh, which builds the
// playground, runs wasm-bindgen and copies this beside the generated pkg/.
//
// Each case runs a Brood snippet through the playground's own `run()` — the same entry
// the site calls — and asserts on the printed result. What is under test is not the
// language (the native suite covers that) but the WASM-ONLY scheduler: the cooperative
// pump, the non-blocking park, the frozen logical clock and `fire_next_timer`.
const pg = require("./pkg/brood_playground.js");

const cases = [
  // A control first: if this fails the harness is broken, not the scheduler.
  { name: "control: the interpreter evaluates at all",
    src: "(+ 1 2)", want: "3" },

  // The pump has to run a spawned process at all — milestone 1.
  { name: "pump: a spawned process runs and its effect is observable",
    src: '(let (me (self)) (do (spawn (send me :ran)) (receive (m m))))',
    want: ":ran" },

  // park -> wake -> resume through the pump, on one thread — milestone 2, the crux.
  { name: "park/wake: send + receive round-trip resumes a parked process",
    src: '(let (me (self) _ (spawn (send me [:hi 42]))) (receive ([:hi n] n)))',
    want: "42" },

  // The pump must sweep EVERY worker queue, not just its own: spawns spill across
  // queues (BROOD_SPAWN_SPILL), so a single-queue pump would hang here.
  { name: "pump sweeps every queue: 3 spawned processes all fan in",
    src: '(defn- w (me i) (send me i))\n(let (me (self)) (do (dotimes (i 3) (spawn (w me i))) (+ (receive (n n)) (receive (n n)) (receive (n n)))))',
    want: "3" },

  // A ping-pong parks and wakes repeatedly — the park path under repetition.
  { name: "park/wake repeats: a 5-message ping-pong completes",
    src: '(defn- echo (me n) (if (= n 0) (send me :done) (do (send me n) (echo me (- n 1)))))\n(defn- drain (n acc) (if (= n 0) acc (drain (- n 1) (+ acc (receive (m (if (= m :done) 0 m)))))))\n(let (me (self)) (do (spawn (echo me 5)) (drain 5 0)))',
    want: "15" },

  // fire_next_timer must advance the FROZEN logical clock, or an `after` never resolves.
  { name: "timer: a receive `after` timeout fires",
    src: '(receive ([:never x] x) (after 5 :timed-out))',
    want: ":timed-out" },

  // Earliest deadline first, in logical time.
  { name: "timer: the earlier of two deadlines wins",
    src: '(list (receive ([:never x] x) (after 50 :late)) (receive ([:never x] x) (after 1 :early)))',
    want: "(:late :early)" },

  // A message must beat a pending timeout: the pump drains run queues before it ever
  // falls through to fire_next_timer.
  { name: "timer: a ready message beats a pending timeout",
    src: '(let (me (self) _ (spawn (send me :message))) (receive (m m) (after 1000 :timed-out)))',
    want: ":message" },

  // Would-block termination — the pump must END, not hang, when nothing can progress.
  { name: "would-block: an unsatisfiable receive terminates instead of hanging",
    src: '(do (receive ([:never x] x)) :unreachable)', wantNot: ":unreachable" },

  // KI: the clock-domain case. The burn spends REAL time without spending LOGICAL time,
  // so a gate that reads `Instant::now()` instead of `sched_now()` sees the second
  // deadline as already passed, re-queues, re-scans, and spins forever. Kept small so
  // the suite stays fast; the by-hand repro script uses a much larger burn.
  { name: "clock domain: real time outrunning logical time still resolves",
    // The burn must take longer in REAL time than the second timeout is in LOGICAL time,
    // or the two clocks never disagree and the case cannot fail. Sized against a measured
    // ~200 ms for 3M iterations: 20M is ~1.3 s of real time against a 50 ms deadline, so
    // the window is ~26x rather than marginal. Verified by sabotage — at 3M vs 200 ms it
    // passed with the bug present, which is the whole reason this comment exists.
    src: '(defn- burn (n acc) (if (= n 0) acc (burn (- n 1) (+ acc 7))))\n(let (a (receive (after 1 :first)) b (burn 20000000 0) c (receive (after 50 :second))) (list a c))',
    want: "(:first :second)" },
];

let failed = 0;
for (const c of cases) {
  const t0 = Date.now();
  let out, err = null;
  try { out = String(pg.run(c.src)); } catch (e) { err = String(e).split("\n")[0]; }
  const ms = Date.now() - t0;
  let ok;
  if (err !== null) ok = false;
  else if (c.wantNot !== undefined) ok = !out.includes(c.wantNot);
  else ok = out.includes(c.want);
  if (!ok) {
    failed++;
    const got = err !== null ? `TRAPPED: ${err}` : JSON.stringify(out);
    const expected = c.wantNot !== undefined ? `NOT ${JSON.stringify(c.wantNot)}` : JSON.stringify(c.want);
    console.log(`FAIL  ${c.name}\n        expected ${expected}\n        got      ${got}`);
  } else {
    console.log(`ok    ${c.name}  (${ms} ms)`);
  }
}
// Always print the summary line: a harness that dies before printing must not be
// mistaken for a clean run (the grep-for-failures trap).
console.log(`\n${cases.length} wasm behavioural cases, ${cases.length - failed} passed, ${failed} failed`);
process.exit(failed === 0 ? 0 : 1);
