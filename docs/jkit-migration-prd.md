# jkit migration — Product Requirements

**Version:** 1.0
**Subcommands of:** `jkit`
**Status:** proposed extension

---

## Purpose

Two new `jkit` subcommands that own the mechanical halves of Flyway migration generation, so the `sql-migration` skill stops improvising DB introspection, schema diffing, NNN-index computation, and Flyway-directory placement in-prompt.

| Subcommand | Owns |
|---|---|
| `jkit migration diff` | DATABASE_URL resolution, DB-type detection, live-schema introspection, target-vs-live diff, NOT-NULL/backfill warnings |
| `jkit migration place` | NNN-index computation (at place-time), file rename, move into Flyway directory, `git add` |

Both are deterministic and reduce the in-prompt failure modes flagged in the skill review:

- False migrations (proposing columns that already exist) — `diff` reads live schema deterministically.
- NNN collisions (computed at generate-time, stale at move-time) — `place` computes NNN at move-time only.
- Missed `git add` — `place` stages explicitly.

**Design principle:** the binary owns introspection, computation, and filesystem mechanics. The model owns judgment-heavy work: writing the *target schema* yaml, writing SQL with backfills/constraints/indexes, and gating the human approval steps.

---

## Shared inputs

| Source | Used by | Purpose |
|---|---|---|
| `pom.xml` | `diff` | Detect DB type from JDBC driver artifact (`postgresql` / `mysql` / `mariadb` / etc.) |
| `$DATABASE_URL` (env or `.env/local*` fallback) | `diff` | Live DB connection |
| `.jkit/<run>/change-summary.md` | `diff` | First-column table identifies affected domains; the binary maps to schemas/tables via convention |
| `.jkit/<run>/target-schema.yaml` | `diff` | Model-authored declarative target state (see schema below) |
| `.jkit/<run>/migration/*.sql` | `place` | Approved SQL ready to move into Flyway dir |
| `src/main/resources/db/migration/*` | `place` | Existing Flyway versions — used to compute the next NNN |

---

## `target-schema.yaml`

The model writes this file before invoking `migration diff`. It describes the *post-migration* state declaratively. The binary diffs it against the live DB to produce the change list.

```yaml
- table: bulk_invoice
  columns:
    - {name: id,         type: uuid,         nullable: false, primary_key: true}
    - {name: tenant_id,  type: uuid,         nullable: false}
    - {name: status,     type: "varchar(32)", nullable: false}
    - {name: created_at, type: timestamptz,  nullable: false, default: now()}

- table: invoice
  columns:
    - {name: bulk_id, type: uuid, nullable: true}
  foreign_keys:
    - {column: bulk_id, references: "bulk_invoice(id)"}
  indexes:
    - {name: idx_invoice_bulk_id, columns: [bulk_id]}
```

**Append-only contract.** The yaml describes what *should exist*, not what's changing — the binary derives "changing" by diffing against live. Existing tables/columns not mentioned are not touched. This means the yaml only needs to enumerate tables/columns relevant to this run.

**Validation.** `diff` validates the yaml on load: malformed → exit 1 with a structured error pointing at the offending entry. Type strings are not checked against the live DB's type system in v1 (left to the human review gate).

---

## `jkit migration diff`

Compute the schema delta and surface warnings.

### CLI

```
jkit migration diff --run <dir> [--no-db] [--pom <path>] [--target <path>]
```

| Argument | Default | Description |
|---|---|---|
| `--run <dir>` | required | Run directory (must contain `change-summary.md` and `target-schema.yaml`) |
| `--no-db` | false | Skip live DB introspection. **Dangerous** — the diff degrades to "everything in target is new." Caller must opt in explicitly. |
| `--pom <path>` | `pom.xml` (cwd) | Maven project file (for DB-type detection) |
| `--target <path>` | `<run>/target-schema.yaml` | Override the target schema location |

### Algorithm

1. Read `pom.xml`, detect DB type from JDBC driver artifact:
   - `org.postgresql:postgresql` → `postgresql`
   - `mysql:mysql-connector-java` / `com.mysql:mysql-connector-j` → `mysql`
   - `org.mariadb.jdbc:mariadb-java-client` → `mariadb`
   - Other / none → exit 1 with `"unsupported or undetected DB type"`.
2. Resolve `DATABASE_URL`:
   1. Process env (already loaded, e.g. via direnv).
   2. `.env/local.env` (single-file layout).
   3. `.env/local/*.env` (directory layout — read all, last `DATABASE_URL` wins).
   4. None → record `db_reachable: false`, reason `"DATABASE_URL not found"`. With `--no-db` continue; without, exit 1.
3. Open live DB connection. Failure → `db_reachable: false`, reason `<error>`. With `--no-db` continue; without, exit 1.
4. Load `target-schema.yaml`. Malformed → exit 1.
5. For each table in the target yaml:
   - Query `information_schema.columns` (and `key_column_usage`, `table_constraints` for FKs/PKs).
   - Compute per-column diff: `add_column`, `alter_column`, or no-op.
   - Compute table-level diff: `create_table` if absent.
   - Compute FK and index diffs.
6. Compute warnings:
   - `add_column` with `nullable: false` and no `default` on a non-empty table → `backfill required for column <table>.<col>`
   - Any `alter_column` changing nullability or type → `existing data may not satisfy constraint`.
7. Emit JSON.

### Output

```json
{
  "db_type": "postgresql",
  "db_reachable": true,
  "target_path": ".jkit/2026-04-25-foo/target-schema.yaml",
  "changes": [
    {
      "kind": "create_table",
      "name": "bulk_invoice",
      "columns": [...],
      "primary_key": ["id"]
    },
    {
      "kind": "add_column",
      "table": "invoice",
      "column": "bulk_id",
      "type": "uuid",
      "nullable": true,
      "foreign_key": "bulk_invoice(id)"
    }
  ],
  "no_op": [
    {"kind": "column_present", "table": "invoice", "column": "tenant_id"}
  ],
  "warnings": [
    "ADD COLUMN bulk_invoice.foo NOT NULL — backfill required (target table is non-empty)"
  ],
  "blocking_errors": []
}
```

| Field | Type | Notes |
|---|---|---|
| `db_type` | string | Detected from pom |
| `db_reachable` | bool | False under `--no-db` or on connection failure |
| `target_path` | string | Echo of resolved `--target` |
| `changes[]` | array | Mutations the migration must perform. Empty → no migration needed. |
| `no_op[]` | array | Items present in target that already match live — useful for reassurance and audit |
| `warnings[]` | string[] | Non-blocking concerns the human should review |
| `blocking_errors` | string[] | Hard problems (e.g. type mismatch the binary refuses to diff) |

### Edge cases

| Case | Behavior |
|---|---|
| `change-summary.md` missing | Exit 1 |
| `target-schema.yaml` missing | Exit 1 with `"sql-migration must produce target-schema.yaml before invoking diff"` |
| Target references a column with non-default type semantics (e.g. `numeric(10,2)`) | Pass-through; live comparison is string-equal on type |
| Target table missing in live DB and `db_reachable: false` | Emit as `create_table` (best-effort under `--no-db`) |
| Target column already in live DB with different type | Emit `alter_column` change + warning |
| Live DB has columns not in target | Reported under `no_op` only if they belong to a target-declared table; truly orphaned columns are ignored (target is append-only relative to the change) |

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success — JSON describes state, including empty `changes` |
| 1 | DB unreachable without `--no-db`, malformed yaml, missing `--run` files, unsupported DB type, I/O error |

---

## `jkit migration place`

Move an approved SQL file into the Flyway directory with a freshly-computed NNN.

### CLI

```
jkit migration place --run <dir> --feature <slug> [--flyway-dir <path>]
```

| Argument | Default | Description |
|---|---|---|
| `--run <dir>` | required | Run directory (must contain exactly one `migration/V*.sql`) |
| `--feature <slug>` | required | Feature slug for the renamed file (used in `V<date>_<NNN>__<slug>.sql`) |
| `--flyway-dir <path>` | `src/main/resources/db/migration/` | Destination directory |

### Algorithm

1. Resolve source: `<run>/migration/V*.sql`. Zero matches → exit 1 (`"no migration SQL staged"`). Multiple → exit 1 (`"more than one migration SQL in run; expected exactly one"`).
2. Scan `<flyway-dir>/V*.sql`, extract the trailing `_NNN` segment from each filename, find the maximum, increment by 1, zero-pad to 3 digits.
3. Compose destination filename: `V<YYYYMMDD>_<NNN>__<slug>.sql` (date = current local date in UTC; matches existing repo convention).
4. Move (rename) the file from `<run>/migration/` to `<flyway-dir>/` with the new name. Atomic where possible.
5. `git add <destination>`. Failure to stage → emit `git_staged: false` in output but exit 0 (caller decides whether to escalate).
6. Emit JSON.

### Output

```json
{
  "source": ".jkit/2026-04-25-foo/migration/V20260425_pending__add_bulk_invoice.sql",
  "destination": "src/main/resources/db/migration/V20260425_003__add_bulk_invoice.sql",
  "next_index": "003",
  "git_staged": true
}
```

### Edge cases

| Case | Behavior |
|---|---|
| `<flyway-dir>` absent | Create it; `next_index = "001"` |
| Filenames in `<flyway-dir>` don't match `V*_NNN__*.sql` | Skip non-matching; max over the matchers; if none match, `next_index = "001"` |
| Source filename doesn't follow `V*_pending*` placeholder | Accept any `V*.sql`; replace the date segment with today's date and the index segment with the computed NNN |
| Destination file already exists | Exit 1 (collision — should not happen under correct skill use) |
| `git add` fails (no git repo, etc.) | `git_staged: false` in output, stderr warning, exit 0 |

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success (including `git_staged: false`) |
| 1 | Zero or multiple source files; destination collision; I/O error |

---

## Suggested dependencies

```toml
# Additions to the existing jkit Cargo.toml
sqlx          = { version = "0.8", features = ["runtime-tokio", "postgres", "mysql"], default-features = false }
serde_yaml    = "0.9"
url           = "2.5"
```

(`sqlx` is heavier than `tokio-postgres` alone, but supports both Postgres and MySQL and gives consistent types across the diff logic. If multi-DB support proves unneeded, drop to a single driver crate.)

---

## Impact on sql-migration

The skill collapses around these calls:

- **Step 1 (Introspect live schema)** + **Step 2 (Write migration-preview.md)** core logic → `jkit migration diff --run <dir>`. The skill writes `target-schema.yaml`, calls the binary, and renders the preview from the diff JSON. Removes the in-prompt `psql` invocation, the env-resolution fallback ladder, and the model's diff computation.
- **Step 3 (Generate migration SQL)** → unchanged at the skill level; the model writes SQL from the diff JSON. Binary doesn't own this — too judgment-heavy (backfills, indexes, type changes).
- **Step 3 NNN computation** → moved to `migration place` (computed at move-time, eliminating the stale-index race).
- **Step 4 (Move to Flyway directory)** → `jkit migration place --run <dir> --feature <slug>`. Owns the move and the `git add`.

Skill responsibilities that remain:
- Reading the spec and writing `target-schema.yaml` (judgment).
- Rendering the markdown preview from diff JSON (presentation).
- Hard-gate prompts (human review).
- Writing migration SQL from approved diff (judgment).
- Bounding the SQL edit-loop (max 3 cycles, then escalate).

Net: ~25 skill lines reclaimable + the false-migration class of bug eliminated (live introspection is now deterministic).
