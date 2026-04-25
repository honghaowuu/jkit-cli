# jkit scenarios — Product Requirements

**Version:** 1.0
**Subcommand of:** `jkit` (Java-specific binary)
**Language:** Rust
**Status:** proposed (split from the previously-proposed standalone `scenarios` binary; the Java-specific halves — `prereqs` and `gap` — live here, while language-agnostic `sync` and `skip` live in `kit scenarios`)

---

## Purpose

Two Java-specific subcommands that own the implementation-loop halves of the test-scenarios pipeline. Both reference Java-specific tooling (Maven pom.xml parsing, Spring Boot version detection, Testcontainers, RestAssured, Java method-name regex, JUnit test-class path conventions). Other language plugins (`gkit`, `tskit`) will provide their own `<lang>kit scenarios prereqs` / `<lang>kit scenarios gap` subcommands with the same shape but language-appropriate implementations.

| Subcommand | Owns |
|---|---|
| `jkit scenarios prereqs` | Detect Spring Boot version, install Java test deps via `jkit pom`, resolve container runtime, ensure compose template is in place |
| `jkit scenarios gap` | List scenarios in `test-scenarios.yaml` that lack a matching JUnit test method, with optional `--run <dir>` aggregation across affected domains |

(Scenario derivation from OpenAPI and per-run skip recording are language-agnostic; see `docs/kit-scenarios-prd.md` for `kit scenarios sync` and `kit scenarios skip`.)

---

## Input files

| File | Used by | Purpose |
|---|---|---|
| `pom.xml` | prereqs (SB-version detection), gap (test_class_path computation) | Java/Maven project metadata |
| `docs/domains/<domain>/test-scenarios.yaml` | gap | Flat YAML scenario manifest (read-only here; written by `kit scenarios sync`) |
| `<run>/change-summary.md` | gap (with `--run`) | First-column table identifies affected domains |
| `<run>/skipped-scenarios.json` | gap (with `--run`) | Per-run skip filter (written by `kit scenarios skip`) |

---

## CLI

```
jkit scenarios prereqs [--apply] [--pom <path>]
jkit scenarios gap <domain> [--test-root <path>]
jkit scenarios gap --run <dir> [--test-root <path>]
```

| Argument | Default | Description |
|---|---|---|
| `<domain>` | required for single-domain `gap` | Domain name (resolves to `docs/domains/<domain>/`) |
| `--run <dir>` | — | Run directory; switches `gap` to multi-domain aggregation |
| `--test-root <path>` | `src/test/java/` | Root directory for JUnit test-file search |
| `--apply` | false | `prereqs` only. Without it, prereqs reports state but mutates nothing. |
| `--pom <path>` | `pom.xml` (cwd) | `prereqs` only |

---

## `prereqs` — detect + install test prerequisites

Owns the Java/Spring-Boot bootstrap phase of the scenario-tdd skill. The binary orchestrates Spring Boot version detection, pom mutation, runtime probing, and template installation so the skill doesn't redraw the decision tree each invocation.

**Delegation.** Pom-fragment installation is delegated to the unified `jkit pom` subcommand (see `docs/jkit-pom-prd.md`). `jkit scenarios prereqs` is a thin orchestrator over `jkit pom` plus non-pom concerns.

### Algorithm

1. Read `pom.xml` (path from `--pom`, default cwd). Parse `<parent><version>` → Spring Boot version. Missing/malformed → exit 1.
2. Pick the testing strategy:
   - SB ≥ 3.1 → `testcontainers`
   - SB < 3.1 → `compose`
3. Pom mutation — invoke `jkit pom prereqs --profile <strategy> [--apply] [--pom <path>]` and capture its JSON. The result populates `pom_status` in this subcommand's output.
4. Strategy-specific non-pom concerns:
   - **testcontainers:** none required.
   - **compose:** probe for container runtime in order: `docker compose`, `docker-compose`, `podman compose`. First hit wins → `runtime`. None found → record blocking error. Verify `docker-compose.test.yml` at repo root; missing → record in `missing_files`. Under `--apply`, copy the bundled template.
5. Without `--apply`: emit JSON describing current state and what *would* be done; mutate nothing.
6. With `--apply`: `jkit pom` mutates the pom; this subcommand handles the compose template copy. Each action is recorded in `actions_taken`.

### Bundled templates

Pom fragments live in `jkit pom` (see its PRD). This subcommand bundles only the non-pom template:

- `templates/docker-compose.test.yml`

Bundled via `include_str!` from `crates/jkit/templates/`.

### Output

Single JSON object to stdout:

```json
{
  "spring_boot_version": "3.2.1",
  "testing_strategy": "testcontainers",
  "runtime": null,
  "pom_status": {
    "profile": "testcontainers",
    "missing": [],
    "already_present": ["testcontainers", "rest-assured", "wiremock"],
    "actions_taken": [],
    "ready": true,
    "blocking_errors": []
  },
  "missing_files": [],
  "actions_taken": [],
  "ready": true,
  "blocking_errors": []
}
```

Field semantics:

| Field | Type | Notes |
|---|---|---|
| `spring_boot_version` | string | From `<parent><version>` |
| `testing_strategy` | `"testcontainers"` \| `"compose"` | Derived from version |
| `runtime` | string \| null | Container runtime (`compose` strategy only); null otherwise |
| `pom_status` | object | Verbatim `jkit pom` response for the chosen profile |
| `missing_files` | string[] | Non-pom templates still missing (e.g. `docker-compose.test.yml`); empty after a successful `--apply` |
| `actions_taken` | string[] | This subcommand's actions (compose-template copy, etc.). `jkit pom`'s actions live under `pom_status.actions_taken`. |
| `ready` | bool | True when this subcommand has no blocking errors AND `pom_status.ready` is true |
| `blocking_errors` | string[] | Human-readable; e.g. `"no container runtime found"` |

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success — JSON describes state. `ready: false` is *not* a failure; the caller decides. |
| `1` | Could not read or parse `pom.xml`; `jkit pom` failed; I/O error. |

---

## `gap` — unimplemented detection

### Algorithm (single domain)

1. Load `docs/domains/<domain>/test-scenarios.yaml` (missing → output `[]`, exit 0).
2. For each scenario, convert `id` kebab-case → camelCase (`happy-path` → `happyPath`, `validation-empty-list` → `validationEmptyList`).
3. Search `*Test.java` under `--test-root` for method declarations matching `void <camelCaseId>\b`.
4. A scenario is **implemented** if at least one match exists.
5. Emit JSON array of `{endpoint, id, description}` for every scenario with no match.

Use a single `grep -rn --include="*Test.java"` call with an alternation pattern across all IDs to minimise filesystem work.

### Output (single domain)

Compact single-line JSON to stdout:

```json
[{"endpoint":"POST /invoices/bulk","id":"happy-path","description":"valid list of 3 → 201 + invoice IDs"}]
```

Empty → `[]`.

### `gap --run <dir>` — aggregate across affected domains

For driving the scenario-tdd implementation loop. Reads `<dir>/change-summary.md` and runs gap detection across every domain listed in its `## Domains Changed` table, returning one ordered JSON array.

**Algorithm:**

1. Open `<dir>/change-summary.md`. Parse the `## Domains Changed` markdown table; affected domains are the first column. Missing file or missing section → exit 1.
2. For each affected domain (in table order), run the standard `gap` algorithm.
3. If `<dir>/skipped-scenarios.json` exists, load it and filter results — drop any `(domain, endpoint, id)` listed there.
4. Augment each remaining entry with two derived fields (see below).
5. Concatenate per-domain results, preserving table order, and emit a single JSON array.

**Augmented entry shape:**

```json
{
  "domain": "billing",
  "endpoint": "POST /invoices/bulk",
  "id": "happy-path",
  "description": "valid list of 3 → 201 + invoice IDs",
  "test_class_path": "src/test/java/com/example/billing/BillingIntegrationTest.java",
  "test_method_name": "happyPath"
}
```

**Derived field rules:**

| Field | Rule |
|---|---|
| `test_method_name` | `camelCase(id)` — same transform `gap` already uses for matching. |
| `test_class_path` | If a file matching `*<DomainPascalCase>IntegrationTest.java` exists under `--test-root`, use that path. Otherwise, compute the default from `pom.xml` `<groupId>` + `<artifactId>` + domain: `<test-root>/<groupPath>/<artifactId>/<domain>/<DomainPascalCase>IntegrationTest.java`. If `pom.xml` is unreadable or lacks groupId, emit `null` and let the caller decide. |

**Exit codes:**

| Code | Meaning |
|---|---|
| `0` | Success (zero gaps emits `[]`) |
| `1` | `<dir>` missing, `change-summary.md` missing/malformed, or any per-domain `test-scenarios.yaml` parse failure |

**No `--pending` flag.** `gap` is already idempotent — it returns only scenarios with no matching test method — so re-running it after partial implementation naturally yields the remaining work. Resume = re-run.

---

## Suggested dependencies

```toml
# Additions to existing jkit Cargo.toml
serde_yaml     = "0.9"           # already present (test-scenarios.yaml)
quick-xml      = { version = "0.36", features = ["serialize"] }   # prereqs: pom.xml parsing
which          = "6"             # prereqs: docker/podman probe
pulldown-cmark = "0.12"          # gap --run: change-summary.md table parse
heck           = "0.5"           # camelCase / kebab-case conversion
```

---

## Impact on scenario-tdd

The Java-specific subcommands collapse two steps of the scenario-tdd skill into single binary calls:

- **Step 1 (Detect SB version + prerequisites)** → `jkit scenarios prereqs --apply`. Replaces the SB-version branching, pom-fragment copying, and runtime resolution. The skill announces results from `actions_taken`.
- **Step 2 (Read affected domains + fetch gaps)** → `jkit scenarios gap --run <dir>`. Replaces the per-domain shell loop and the model improvising `test_class_path` / `test_method_name` from endpoint strings.

(Step 3 lightweight gate "Skip" branch uses `kit scenarios skip --run <dir>` — language-agnostic, see `docs/kit-scenarios-prd.md`.)
