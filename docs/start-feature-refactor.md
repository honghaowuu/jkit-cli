# start-feature refactor — Rust binary work

**Status:** markdown side complete in `/workspaces/jkit`. This file is the brief for the matching binary work in this repo (`jkit-cli`).

**Context:** the jkit plugin's spec phase was previously three skills (`brainstorming` → `write-change` → `spec-delta`) producing a change file in `docs/changes/pending/`, plus a derived `.jkit/<run>/change-summary.md`. That collapsed into one user-invocable skill, **`/start-feature`**, with one artifact (`.jkit/<run>/design.md`) on a per-feature `feature/<slug>` git branch. The `docs/changes/pending/` and `done/` inboxes are gone — branches replace them. The pre-existing `jkit changes` subcommand surface goes away with them.

This refactor is in the plugin's markdown but **not yet implemented in this binary**. The plugin currently references CLI commands that don't exist — `jkit feature start` and `jkit feature init`. Until they ship, the plugin will fail at runtime when `/start-feature` is invoked.

## What needs to change

### 1. Add `jkit feature` subcommand group

Two subcommands.

#### `jkit feature start`

**Purpose:** detect git branch state + run the domain doctor. Combined entry-routing call for `/start-feature` Step 1.

**Inputs:** none (operates on cwd).

**Logic:**
1. Read current branch via `git symbolic-ref --short HEAD` (or equivalent).
2. Branch routing:
   - `main` (or whatever `init.defaultBranch` is — read from git config) AND working tree clean → create branch `feature/<placeholder-YYYY-MM-DD-HHMMSS>` off `main`, check it out, return `status: "fresh"`.
   - `feature/*` → return `status: "resume"`. Find the matching `.jkit/<run>/` dir (there should be exactly one whose slug matches the branch suffix; if not, return `status: "resume"` with `run_path: null` and let the skill ask the user).
   - `main` AND working tree dirty → return `status: "dirty"` — the skill prompts the user (stash / commit / abort).
   - Other branches → return `status: "off_main"` with a hint message; skill halts.
3. Run domain doctor (logic equivalent to today's `jkit changes doctor` minus the pending/done cross-checks — keep `domains.yaml` ↔ `test-scenarios.yaml` slug consistency, malformed entries, etc.).

**Output (JSON envelope):**
```json
{
  "branch": "feature/<slug>" | "main" | "<other>",
  "status": "fresh" | "resume" | "dirty" | "off_main",
  "doctor_ok": true | false,
  "doctor_findings": [
    {"severity": "issue" | "warning", "message": "...", "remediation": "..."}
  ],
  "run_path": ".jkit/<run>/" | null
}
```

**Notes:**
- Idempotent on `feature/*` branches: re-running just re-detects the existing branch.
- On `fresh`, the placeholder branch name is intentionally throwaway — `jkit feature init` renames it to `feature/<final-slug>` once the slug is decided.
- Doctor with `severity: "issue"` blocks the skill from continuing; the skill halts and surfaces findings.

#### `jkit feature init --title "<title>" --domains <comma-list>`

**Purpose:** create the run dir + rename the placeholder branch + validate frontmatter. `/start-feature` Step 5.

**Inputs:**
- `--title` (required): short imperative title. Slug derived as kebab-case from the title (lowercase, alphanumeric + hyphens).
- `--domains` (required): comma-separated list of domain slugs.

**Logic:**
1. Derive slug from title.
2. Compute run dir: `.jkit/YYYY-MM-DD-<slug>/` using today's date.
3. Validate `domains[]` against `docs/domains.yaml` — every slug must exist there. On any unknown slug, fail with a clear error.
4. Check current branch is a `feature/*` branch (i.e., `feature start` was already run in this session). If not, fail with a hint to run `feature start` first.
5. Check for collision: if `.jkit/YYYY-MM-DD-<slug>/` already exists in git history (any branch / merged into main), fail loudly. Pick a different title.
6. Create the run dir.
7. Rename current branch to `feature/<slug>` via `git branch -m feature/<slug>`.

**Output (JSON envelope):**
```json
{"run_path": ".jkit/2026-04-30-<slug>/"}
```

**Notes:**
- The skill writes `<run_path>/design.md` itself after this returns — the binary just creates the dir and renames the branch.
- Idempotent on identical input: re-running with the same title + domains on an already-renamed branch is a no-op (verify dir exists + branch matches).

### 2. Modify `sql-migration`-side support

Today, `/sql-migration` is the schema branch — invoked only when spec-delta detected schema changes. With branches, `/sql-migration` is **always** invoked from `/start-feature`, and decides for itself whether there's work to do:

- Scan `*Entity.java` files referenced by the affected domains (from `<run>/design.md` frontmatter `domains: [...]`).
- If no entities changed → log a pass-through message and tail-call `writing-plans` with no commit.
- Otherwise → existing flow (target-schema → migration diff → SQL → place → migration commit).

The Rust side may need a small new helper if the entity-scan logic isn't already exposed as a CLI surface. The skill currently does this scan inline by reading entity files — that may stay in the skill or move into a `jkit migration scan-entities --run <run>` subcommand. Decide based on what's cleaner. **Token-efficiency principle: if the inline scan is more than ~10 lines of skill prose / multiple file reads, push it into the binary.**

### 3. Remove the deprecated `jkit changes` subcommand group

Drop entirely:
- `jkit changes bootstrap`
- `jkit changes status`
- `jkit changes doctor` (its useful parts — domain consistency — fold into `jkit feature start`)
- `jkit changes validate`
- `jkit changes init`
- `jkit changes summary`
- `jkit changes complete`

The plugin no longer references any of these. Clean removal — no compatibility shim.

### 4. Other small touch-ups

- `jkit init` no longer needs to create `docs/changes/{pending,done}/`. It still scaffolds `docs/domains.yaml`, `docs/test-scenarios.yaml`, `docs/overview.md`, `.envrc`, `.env/local.env`, `.env/test.env`, `docker-compose.yml`, `.gitignore` block.
- `.gitignore` block: drop `.jkit/done/` from the gitignore (no archive dir anymore). `.jkit/adhoc-*/` stays.
- Anywhere the binary reads `<run>/change-summary.md`, switch to `<run>/design.md`. Frontmatter now carries `feature` + `domains: [...]` (no `assumptions`, no `schema_changes` — those got dropped during the refactor).
- `jkit scenarios gap --run <dir>` previously parsed change-summary's `## Domains Changed` table for affected domains. Switch to reading `design.md` frontmatter `domains: [...]`.
- `jkit coverage --scope run:<dir>` same change.

## Validation criteria

After implementation:

```bash
cd /workspaces/jkit-cli
cargo build --release
cargo test --lib
```

Both must pass cleanly. Then vendor:

```bash
cp target/release/jkit /workspaces/jkit/bin/jkit-linux-x86_64
```

Smoke test the new commands against a scratch project — at minimum:
- `jkit feature start` on a clean main → creates a `feature/<placeholder>` branch
- `jkit feature start` on that branch → returns `status: "resume"`
- `jkit feature init --title "Test feature" --domains <existing-slug>` → creates `.jkit/<date>-test-feature/`, renames branch
- `jkit feature init --domains <unknown-slug>` → fails loudly

## Commit hygiene

Per `/workspaces/jkit/CLAUDE.md`'s vendoring rule:
> Commit the binary and the matching SKILL.md prose together — never ship a binary without the markdown update describing the new behavior.

The SKILL.md changes already landed in `/workspaces/jkit` (the prose is correct, the binary just doesn't exist yet). Two commits to coordinate:

1. **`/workspaces/jkit-cli`:** the Rust changes + updated tests. Suggested subject: `feat: jkit feature start/init for /start-feature pipeline`. Reference this brief.
2. **`/workspaces/jkit`:** vendor the binary into `bin/jkit-linux-x86_64`. Suggested subject: `bin: vendor jkit with feature start/init (start-feature refactor)`.

## Reference: where the contract is documented

- `/workspaces/jkit/skills/start-feature/SKILL.md` — caller of `jkit feature start|init`
- `/workspaces/jkit/skills/sql-migration/SKILL.md` — pass-through path (Step 0a)
- `/workspaces/jkit/skills/scenario-tdd/SKILL.md`, `/workspaces/jkit/skills/java-tdd/SKILL.md`, `/workspaces/jkit/skills/java-coverage/SKILL.md` — design.md consumers
- `/workspaces/jkit/docs/LOGIC.md` §3.1 — full subcommand reference (the part labeled "`jkit feature start`" / "`jkit feature init`" describes the JSON envelopes a fresh implementor should match)
- `/workspaces/jkit/docs/GUIDE.md` §3.2 — chain shape
