# codexbar CLI timings (Linux, v0.20)

Host: Arch / omarchy, Linux kernel default, NVMe. Wall-clock measurements via bash `date +%s.%N` around a silent invocation (`>/dev/null 2>&1`). Each run spawns a fresh process — no shared daemon, no prewarm.

## Cold + warm runs

### `codexbar usage --format json` (default provider=all → codex-only per config, --source=auto → errors on Linux)

```
run 1: 0.010s
run 2: 0.011s
run 3: 0.011s
run 4: 0.010s
run 5: 0.010s
mean=0.010s  min=0.010s  max=0.011s
```

This case short-circuits at the "web source requires macOS" guard, so it is not representative. It does tell us that **cold process startup + JSON error emission is ~10 ms** — that's the fixed overhead of every invocation.

### `codexbar usage --provider claude --source cli --format json` (the actual TUI path)

```
run 1: 15.132s
run 2: 15.118s
run 3: 14.985s
run 4: 14.882s
run 5: 14.983s
mean=15.020s  min=14.882s  max=15.132s
```

Plus 3 additional back-to-back runs after a warm-up call:

```
run 1: 15.101s
run 2: 15.043s
run 3: 15.180s
```

Conclusion: **~15 s per call, no meaningful cache.** The cost is dominated by launching the `claude` CLI into a PTY, which itself has multi-second Node startup + a `/usage` round trip to Anthropic.

### `codexbar cost --provider claude --format json`

```
run 1: 15.746s
run 2: 15.678s
run 3: 15.742s
run 4: 15.869s
run 5: 15.637s
mean=15.734s  min=15.637s  max=15.869s
```

Back-to-back after a warm-up:

```
run 1: 15.624s
run 2: 15.754s
run 3: 15.764s
```

### `codexbar cost --provider claude --refresh --format json`

```
run 1: 15.673s
run 2: 15.784s
run 3: 15.666s
```

The `--refresh` flag does not materially change the cost — strong evidence that the `cost` command rescans `~/.claude/projects/**/*.jsonl` unconditionally on this build, and the 15 s is the scan itself (our `~/.claude/projects/` is `>1 MB history.jsonl` + a big `file-history/` tree and hundreds of project dirs — lots of JSONL to walk).

## Implications for the TUI

1. **Never block the render loop on a codexbar call.** 15 s of blank TTY would be unusable.
2. **Poll floor: 30 s.** Polling faster than the call duration + a safety margin just queues refreshes. Default 60 s.
3. **Separate schedules for `usage` and `cost`.** Cost numbers move in daily buckets; polling cost every 5 min is plenty.
4. **Single in-flight guarantee per command.** Use a `tokio::sync::Mutex` (or a "request in progress" flag) so overlapping refreshes don't pile up.
5. **Child-process timeout: 30 s.** 2× observed p100. Kill + log + keep the previous snapshot.
6. **Consider trimming `~/.claude/projects/`** for the user in our docs — if their scan is 30 s+, the TUI will feel even worse. That is a user-side fix, not a TUI-side one.

## Raw commands used

```
for i in 1..5: time codexbar usage --format json
for i in 1..5: time codexbar usage --provider claude --source cli --format json
for i in 1..5: time codexbar cost --provider claude --format json
for i in 1..3: time codexbar cost --provider claude --refresh --format json
```

All redirected to `/dev/null`; timing was the wall-clock delta between `date +%s.%N` markers.
