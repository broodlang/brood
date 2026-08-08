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

## The flake, and how to re-check the fix

KI-38 is **fixed** (`scripts/warm-boot-cache.sh`, wired as a nextest setup
script). What follows is how it was reproduced, kept because it doubles as the
regression check — the fix is what stands between these numbers and a red suite.

To reproduce the ORIGINAL failure, disable the warm-up as well as the cache:

```sh
rm -f ~/.cache/brood/prelude-expanded-*.blsp
BROOD_NO_WARM_BOOT_CACHE=1 \
  cargo nextest run --no-fail-fast --features brood/treesit-grammars -j 64
```

(There is no nextest-side way to skip a setup script — `--config
'profile.default.scripts=[]'` does *not* disable it, verified — hence the env
var, named after `BROOD_NO_BOOT_CACHE` which disables the cache itself.)

`clean_peer_exit_fires_nodedown_promptly` fails at 20.119 s in
`wait_until_listening`, then passes on retry (warm by then). With the setup
script left enabled, the same command puts it at ~2.6 s.

`-j 64` on 12 cores independently breaks `gc spawned_process_reclaims_too` and
times out a jit case — over-subscription damage, not a regression; both pass at
the default `-j`.
