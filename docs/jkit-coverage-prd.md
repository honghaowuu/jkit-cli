# jkit coverage — Product Requirements

**Version:** 1.1
**Subcommand of:** `jkit` (Java-specific binary)
**Language:** Rust
**Status:** proposed (replaces the previously-standalone `jacoco-filter` v1.0 binary; folded into `jkit` for one-binary-per-language consistency)

---

## Background

The previous `jacoco-filter` v1.0 binary read a JaCoCo XML report and emitted a ranked list of methods with coverage gaps. It was invoked from `java-tdd` Step 5 to drive the unit-test coverage loop. This PRD describes its v1.1 reincarnation as the `jkit coverage` subcommand.

v1.1 adds **iteration state tracking (`--iteration-state` flag)** — detect when the coverage-fill loop has plateaued. Currently a model bookkeeping task ("if two consecutive passes produce no decrease in missed lines, stop") that drifts.

**Note on plugin bootstrap.** An earlier draft of v1.1 also included a `prereqs` subcommand for installing the JaCoCo Maven plugin in `pom.xml`. That responsibility moved to the unified `jkit pom` subcommand (see `docs/jkit-pom-prd.md`) so all pom mutations across the pipeline go through one schema and template set. Callers needing the plugin installed invoke `jkit pom prereqs --profile jacoco --apply` directly.

**Design principle (carried from v1.0):** structured input, structured output. `--iteration-state` augments existing output with plateau-detection fields.

---

## CLI change

### New flag on existing call: `--iteration-state`

```
jkit coverage <jacoco.xml> [--summary] [--min-score <f>] [--top-k <n>] [--iteration-state <path>]
```

Without the flag, behavior is unchanged from v1.0.

---

## `--iteration-state` — flag

Tracks missed-line totals across successive coverage-loop passes so the skill can stop when no progress is being made.

### Algorithm

1. If `--iteration-state <path>` is provided:
   - Read existing JSON state if present. Malformed → stderr warning, treat as absent.
   - Compute `missed_lines_total` = sum of `missed_lines.length` across all methods in the current report (after standard v1.0 filtering).
   - Append `{"timestamp": "<ISO8601>", "missed_lines_total": N}` to `iterations`.
   - Compute `missed_lines_delta` = current total minus previous total (`null` on first iteration).
   - Compute `consecutive_no_progress` = count of trailing iterations where `delta >= 0` (no decrease).
   - Set `should_stop` = `consecutive_no_progress >= 2`.
   - Write the updated state file (atomic: write to tempfile in same dir, rename).
2. Augment the standard output JSON with the iteration fields. Do not modify any v1.0 fields.

### State file shape

```json
{
  "iterations": [
    {"timestamp": "2026-04-25T14:00:00Z", "missed_lines_total": 24},
    {"timestamp": "2026-04-25T14:05:00Z", "missed_lines_total": 18},
    {"timestamp": "2026-04-25T14:10:00Z", "missed_lines_total": 18}
  ]
}
```

### Augmented output

```json
{
  "summary": { ... },
  "methods": [ ... ],
  "iteration": 3,
  "missed_lines_total": 18,
  "missed_lines_delta": 0,
  "consecutive_no_progress": 1,
  "should_stop": false
}
```

When `--iteration-state` is *not* passed, none of the four iteration fields are emitted (strict v1.0 compatibility).

### Edge cases

| Case | Behavior |
|---|---|
| State file missing | Create it. `iteration: 1`, `missed_lines_delta: null`, `consecutive_no_progress: 0`, `should_stop: false` |
| State file malformed | Stderr warning, treat as absent (start fresh) |
| State file unwritable | Exit 1 — caller asked for tracking; failing silently is wrong |
| `methods: []` (full coverage, missed_lines_total = 0) | Normal delta logic. Two consecutive zeros → `should_stop: true` (loop is done — also means "you reached coverage goal") |
| State file from a different project (different total methods than current) | No special handling; tracked by missed-line count alone, which is sufficient |
| `consecutive_no_progress` boundary: first pass after a decrease then plateau | After 1 decrease + 1 plateau, `consecutive_no_progress: 1`, `should_stop: false`. After 2 plateaus, `should_stop: true`. |

### Why missed-line count, not method count?

Method count drops to zero only when all methods are perfectly covered. Missed-line count drops gradually as tests are added, giving finer-grained progress signal and detecting plateau earlier.

---

## Backwards compatibility

- All existing v1.0 invocations work unchanged.
- The augmented output fields are present **only** when `--iteration-state` is passed.

---

## Suggested dependencies

```toml
# Additions to the existing v1.0 Cargo.toml
chrono = { version = "0.4", features = ["serde"] }   # iteration-state: timestamps
```

If v1.0 already depends on `serde_json` and `serde`, no further additions for state-file I/O.

---

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success (including zero gaps) |
| 1 | Hard error — XML parse failure, state file unwritable, I/O failure |

---

## Impact on java-tdd

- **Step 5 iteration bound** → add `--iteration-state .jkit/<run>/coverage-state.json` to the `jkit coverage` call. The skill rule collapses from "if two consecutive passes produce no decrease in missed lines, stop" to "if `should_stop: true`, stop." Accuracy win on plateau detection (most prone to model drift).

(Step 3 prerequisites — JaCoCo plugin install — moved to `jkit pom prereqs --profile jacoco --apply`. See `docs/jkit-pom-prd.md`.)
