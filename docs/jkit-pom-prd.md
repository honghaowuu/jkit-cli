# jkit pom — Product Requirements

**Version:** 1.0
**Subcommand of:** `jkit` (Java-specific binary)
**Language:** Rust
**Status:** proposed (replaces the previously-proposed standalone `pom-doctor` binary; folded into `jkit` for one-binary-per-language consistency)

---

## Purpose

A deterministic subcommand that owns **all** `pom.xml` mutations for the jkit pipeline. Higher-level subcommands (`jkit scenarios prereqs`, `jkit contract stage`) and skills (`java-tdd`, `java-verify`, `scenario-tdd`) delegate pom-checking and pom-fragment installation to `jkit pom` so:

1. There is **one** XML parser, one mutation engine, one set of bundled fragment templates — no drift across consumers.
2. The error model and output schema are uniform — every caller consumes the same JSON shape.
3. Pom mutation, the most error-prone in-prompt operation in the pipeline, lives in a single typed binary with tests.

**Design principle:** `jkit pom` is a primitive. It does not detect Spring Boot versions, probe runtimes, or copy non-pom files. Composite operations (e.g. "set up scenario testing for this project") live in higher-level subcommands that compose `jkit pom` with their own concerns.

---

## CLI

```
jkit pom prereqs --profile <profile> [--apply] [--pom <path>]
jkit pom add-dep --group-id <gid> --artifact-id <aid> --version <ver> [--scope <scope>] [--apply] [--pom <path>]
```

### `prereqs` — static-profile installation

| Argument | Default | Description |
|---|---|---|
| `--profile <profile>` | required | One of: `testcontainers`, `compose`, `jacoco`, `quality`, `smart-doc` |
| `--apply` | false | Without it: report state, mutate nothing. With it: install missing fragments. |
| `--pom <path>` | `pom.xml` (cwd) | Path to the Maven project file |

### `add-dep` — dynamic single-dependency add

For deps that aren't part of a fixed profile (e.g. an SDK chosen at runtime by a skill).

| Argument | Default | Description |
|---|---|---|
| `--group-id <gid>` | required | Maven groupId |
| `--artifact-id <aid>` | required | Maven artifactId |
| `--version <ver>` | required | Maven version |
| `--scope <scope>` | (omitted from pom) | Maven scope (`compile`, `test`, `provided`, etc.) |
| `--apply` | false | Without it: report state. With it: insert into `<dependencies>`. |
| `--pom <path>` | `pom.xml` (cwd) | Path to the Maven project file |

---

## Profiles

Each profile names a pom-fragment bundle the subcommand knows about. Templates are compiled into the `jkit` binary via `include_str!` from `crates/jkit/templates/pom-fragments/`.

| Profile | Fragments | Inserts under |
|---|---|---|
| `testcontainers` | Testcontainers JUnit 5 + PostgreSQL + RestAssured + WireMock + Spring Boot Testcontainers ServiceConnection | `<dependencies>` |
| `compose` | RestAssured | `<dependencies>` |
| `jacoco` | jacoco-maven-plugin (prepare-agent + report goals) | `<build><plugins>` |
| `quality` | Spotless (google-java-format) + PMD + SpotBugs | `<build><plugins>` |
| `smart-doc` | smart-doc-maven-plugin (used by `jkit contract stage` for OpenAPI generation) | `<build><plugins>` |

`testcontainers` and `compose` are mutually exclusive — callers pick based on Spring Boot version. The binary does not detect.

---

## Algorithm — `prereqs`

1. Read `pom.xml` (path from `--pom`, default cwd). Parse error → exit 1.
2. Resolve the profile to its fragment list. Unknown profile → exit 1 with valid choices.
3. For each fragment:
   - Locate the parent element (`<dependencies>` or `<build><plugins>`). Missing parent under `--apply`: create it. Missing parent in dry-run: count as a missing fragment, no error.
   - Look for the fragment by `groupId+artifactId` (deps) or `<plugin>` with matching `<artifactId>` (plugins).
   - Present → record in `already_present`.
   - Missing → record in `missing`. Under `--apply`, insert the bundled fragment (atomic write — tempfile in same dir, rename) and record in `actions_taken`.
4. Emit JSON.

Indentation: detect from the existing pom (most common indent unit in the document) and match. Default to 4 spaces if undetectable.

## Algorithm — `add-dep`

1. Read `pom.xml`. Parse error → exit 1.
2. Locate `<dependencies>`. Missing under `--apply`: create directly under `<project>`. Missing in dry-run: report as `dependencies_section_missing` and continue.
3. Search for an existing dep matching `groupId + artifactId`:
   - Same version → record `present: true`, `action: "skipped"`.
   - **Different version** → record `present: true`, `action: "version_mismatch"`, surface in `warnings`. **Never modify** — existing config is authoritative.
   - Absent → record `present: false`. Under `--apply`, insert a new `<dependency>` block with the supplied groupId/artifactId/version (and `<scope>` if `--scope` is set) and record in `actions_taken`.
4. Emit JSON.

Output uses the same shape as `prereqs` output but with a single-fragment `fragments[]` entry. The `--scope` value, if provided, is included in the inserted fragment verbatim.

---

## Output

Single JSON object to stdout:

```json
{
  "profile": "jacoco",
  "fragments": [
    {"id": "jacoco-maven-plugin", "present": false, "action": "added"}
  ],
  "missing": [],
  "already_present": [],
  "actions_taken": ["added jacoco-maven-plugin to <build><plugins>"],
  "ready": true,
  "blocking_errors": []
}
```

| Field | Type | Notes |
|---|---|---|
| `profile` | string | Echo of the requested profile |
| `fragments[]` | array | Per-fragment status. `action` is one of `"added"`, `"skipped"` (already present), `"reported"` (dry-run) |
| `missing` | string[] | Fragment ids missing **after** the call (empty after a successful `--apply`) |
| `already_present` | string[] | Fragment ids skipped because already configured |
| `actions_taken` | string[] | Empty in dry-run; one entry per mutation under `--apply` |
| `ready` | bool | True when `missing` is empty and `blocking_errors` is empty |
| `blocking_errors` | string[] | Human-readable; e.g. `"pom.xml has no <project> root element"` |

---

## Edge cases (apply to both modes)

| Case | Behavior |
|---|---|
| `pom.xml` missing | Exit 1 |
| `pom.xml` has no `<build>` and profile is jacoco/quality/smart-doc | `--apply`: create `<build><plugins>`, insert. Dry-run: report as missing. |
| `pom.xml` has no `<dependencies>` (any dep insertion) | Same — create under `--apply`, list as missing in dry-run. |
| Plugin/dep present with non-default config (version override, executions, scope) | Treat as present; never modify existing config. `add-dep` with version mismatch surfaces in `warnings`. |
| Multi-module pom (parent with `<modules>`) | Operate only on the file passed to `--pom`; warn to stderr `"multi-module project — verify this is the right pom"` |
| `--apply` write fails (permissions, FS full) | Exit 1 with error |
| Unknown `--profile` | Exit 1 with list of valid profiles |
| `add-dep` with empty groupId/artifactId/version | Exit 1 |
| Pom uses tabs vs spaces | Match the dominant style in the original |

---

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success — JSON describes state. `ready: false` is *not* a failure; the caller decides. |
| 1 | Could not read or parse `pom.xml`; could not write under `--apply`; unknown profile; I/O error |

---

## Suggested dependencies

```toml
[dependencies]
serde      = { version = "1", features = ["derive"] }
serde_json = "1"
clap       = { version = "4", features = ["derive"] }
quick-xml  = { version = "0.36", features = ["serialize"] }
```

---

## Impact on other binaries

### `jkit scenarios prereqs`

Currently does: SB version detection + pom mutation (testcontainers or compose) + runtime probing + compose template copying.

Under this change, scenarios becomes a thin orchestrator:

1. SB version detection — stays in scenarios.
2. Pom mutation — delegates to `jkit pom prereqs --profile testcontainers` or `--profile compose`.
3. Runtime probing — stays in scenarios.
4. Compose template copying — stays in scenarios.

The `jkit scenarios prereqs` output JSON gains a `pom_status` key carrying `jkit pom`'s response verbatim, alongside the existing `runtime` / `missing_files` / etc. fields.

### `jkit coverage` and the former standalone `jacoco-filter`

The previously-proposed `jacoco-filter prereqs` subcommand is folded into `jkit pom prereqs --profile jacoco`. The remaining JaCoCo functionality (filtering, scoring, `--iteration-state`) lives in `jkit coverage` — see `docs/jkit-coverage-prd.md`.

### `bin/pom-add.sh` — **removed**

The previous shell-based pom mutator was deleted alongside this PRD landing. Quality, jacoco, and testcontainers fragments are all now sourced from `jkit pom`'s bundled templates.

---

## Impact on skills

- **java-tdd Step 3** → `jkit pom prereqs --profile jacoco --apply`.
- **java-verify Step 1** → `jkit pom prereqs --profile quality --apply`.
- **scenario-tdd Step 1** → unchanged at the skill level (continues to call `jkit scenarios prereqs`); the orchestration shift is internal to that subcommand.
- **publish-contract Step 5 (stage)** → `jkit pom prereqs --profile smart-doc --apply` (delegated by `jkit contract stage`).
- **generate-feign SDK opt-in** → `jkit pom add-dep --group-id ... --artifact-id ... --version ... --apply`.

Net architectural effect: every pom-mutation point in the pipeline goes through one binary, one schema, one bundled-template set, one error model.
