# jkit

Java-specific developer-pipeline CLI. A single Rust binary that owns the deterministic, error-prone halves of a Java/Spring/Maven workflow so model-driven skills don't have to improvise pom mutation, schema diffing, controller scanning, or coverage-loop bookkeeping.

Implements seven subcommands described under [`docs/`](docs/):

| Subcommand | Owns |
|---|---|
| `jkit pom`        | All `pom.xml` mutations — static profile installs and one-off dep adds |
| `jkit coverage`   | JaCoCo XML filtering + ranking, plus iteration-state plateau detection |
| `jkit scenarios`  | Spring Boot version detection, test prereq install, scenario gap audit |
| `jkit drift`      | Bi-directional controller-vs-`api-spec.yaml` drift detection (used by `/post-edit`) |
| `jkit migrate`    | Reverse-engineer `docs/domains/<n>/` skeletons from existing `@RestController` classes (legacy onboarding) |
| `jkit migration`  | Live-DB introspection + target-schema diff, Flyway file placement |
| `jkit contract`   | Service metadata extraction, smart-doc orchestration, contract bundle staging |

## Build

Requires Rust 1.75+ (workspace pins `edition = 2021`).

```sh
cargo build --release
./target/release/jkit --help
```

## Usage

### `pom`

Static profiles install a known set of dependencies or plugins:

```sh
# Dry-run: report state, mutate nothing
jkit pom prereqs --profile jacoco

# Apply: insert missing fragments
jkit pom prereqs --profile jacoco --apply
```

Profiles: `testcontainers`, `compose`, `jacoco`, `quality`, `smart-doc`.

One-off dependency add (used e.g. by `generate-feign` for SDK opt-in):

```sh
jkit pom add-dep \
  --group-id io.example --artifact-id billing-api --version 1.2.0 \
  --scope compile --apply
```

If the dep already exists at a different version, the call refuses to modify and surfaces a `version_mismatch` warning — existing config is authoritative.

### `coverage`

Filter JaCoCo XML, drop trivial getters/setters and fully-covered methods, score the remainder by `complexity × (missed/total)`:

```sh
jkit coverage target/site/jacoco/jacoco.xml --top-k 5 --summary
```

Add `--iteration-state` to track plateau in a coverage-fill loop. The state file accumulates `missed_lines_total` per pass; after two consecutive passes with no decrease, the output reports `should_stop: true`:

```sh
jkit coverage target/site/jacoco/jacoco.xml \
  --iteration-state .jkit/run-2026-04-25/coverage-state.json
```

### `scenarios`

Detect Spring Boot version, install matching test deps via `jkit pom`, ensure a container runtime + compose template if needed:

```sh
jkit scenarios prereqs --apply
```

List scenarios in `docs/domains/<domain>/test-scenarios.yaml` that lack a JUnit test method:

```sh
# Single domain
jkit scenarios gap billing

# All affected domains in a run, with test_class_path/test_method_name resolved
jkit scenarios gap --run .jkit/run-2026-04-25
```

### `migration`

Diff a model-authored `target-schema.yaml` against a live database (Postgres or MySQL/MariaDB):

```sh
jkit migration diff --run .jkit/run-2026-04-25
```

`DATABASE_URL` is read from process env, then `.env/local.env`, then `.env/local/*.env` (last wins). `--no-db` degrades to "everything in target is new".

Move an approved SQL file into the Flyway directory with a freshly-computed `NNN`:

```sh
jkit migration place --run .jkit/run-2026-04-25 --feature add_bulk_invoice
# → src/main/resources/db/migration/V20260425_003__add_bulk_invoice.sql
# Also runs `git add` on the destination.
```

### `contract`

Read-only service metadata (used by the publish-contract skill to draft prompts):

```sh
jkit contract service-meta
```

Stage a contract bundle in `.jkit/contract-stage/<service>/`:

```sh
jkit contract stage \
  --service billing \
  --interview .jkit/run-2026-04-25/interview.json \
  --domains invoice,bulk-invoice
```

The stage step delegates smart-doc install to `jkit pom`, runs `mvn smart-doc:openapi`, converts the resulting JSON to `contract.yaml`, instantiates `plugin.json` + `SKILL.md` + `domains/*.md` from bundled Tera templates, and adds `.jkit/contract-stage/` to `.gitignore`.

## Output contract

Every subcommand emits a single JSON object (or array, for `scenarios gap`) on stdout. Errors go to stderr; exit code 1 for hard failures (parse errors, missing required files, unwritable destinations). `ready: false` on a successful call is *not* an error — the caller decides.

## Layout

```
crates/jkit/
├── src/
│   ├── main.rs            # clap routing
│   ├── lib.rs
│   ├── pom/               # prereqs, add-dep, format-preserving XML mutation
│   ├── coverage/          # JaCoCo parse/filter/score + iteration state
│   ├── scenarios/         # prereqs (delegates to pom), gap detection
│   ├── migration/         # diff (sqlx introspection), place (NNN + git add)
│   ├── contract/          # service-meta, stage (smart-doc + Tera templates)
│   └── util.rs            # atomic_write, print_json
├── templates/
│   ├── pom-fragments/     # bundled via include_str!
│   ├── contract/          # Tera templates: plugin.json, SKILL.md, domain.md, smart-doc.json
│   └── docker-compose.test.yml
└── tests/
    ├── coverage_integration.rs
    ├── pom_integration.rs
    └── fixtures/sample.xml
```

## Tests

```sh
cargo test
```

Currently 29 unit tests + 16 integration tests covering: XML indent detection, format-preserving inserts, version-mismatch handling, JaCoCo parser correctness, plateau detection, kebab/camel transforms, markdown table parsing, type-equivalence aliases, NNN computation, javadoc quality scoring, Spring Boot version dispatch.

`mvn`-driven (`contract stage`) and live-DB (`migration diff`) flows aren't covered by tests in this repo — they need an actual Maven project / DB. Smoke them by hand against a real service.

## Status

Each subcommand corresponds to a numbered PRD under [`docs/`](docs/). Behavior is intended to track those PRDs exactly; if you find drift, the PRD is the source of truth.
