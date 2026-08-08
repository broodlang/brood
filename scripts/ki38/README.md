# KI-38 harness — the cold-boot-cache flake

Committed because the previous session's equivalent tooling lived in a session
scratchpad and its figures could not be re-derived (see `docs/handoff.md`).

- `bootcost.sh [N]` — what a **cold** expanded-prelude boot costs a debug
  `brood`, alone and as a herd of N, against the warm path. Isolated via
  `XDG_CACHE_HOME`, so it never touches the real `~/.cache/brood`.
- `doseresponse.py` — sweeps herd size against a shared cold cache and reports
  the worst boot in each herd: the number the 20 s / 30 s deadlines race.
  Samples RSS and aborts the sweep before the box is put at risk.
- `sysmon.py OUT.csv` — 1 Hz timeline beside a suite run (loadavg,
  MemAvailable, SwapFree, Shmem, major-fault/swap deltas, live `brood` count
  and how many sit in `D`, and which test binary is running so the timeline
  names the schedule region). Reads `/proc` only and spawns no `brood`, so it
  cannot re-create the KI-29 orphan leak an earlier sampler did.
- `analyze.py OUT.csv` — per-round envelope, the release_bundle region, the
  tightest-memory moment, and every D-state sighting. Runs on **green** rounds
  too, which is the point: a green round still shows whether pressure exists.

Reproduce the flake on demand (12-core box):

```sh
rm -f ~/.cache/brood/prelude-expanded-*.blsp
cargo nextest run --no-fail-fast --features brood/treesit-grammars -j 64
```

`-j 64` on 12 cores also breaks `gc spawned_process_reclaims_too` and times out
3 cases — over-subscription damage, not a regression.
