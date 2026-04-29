//! `proposed-api.yaml` — the per-run review artifact authored by
//! `writing-plans` for any change that touches the public API.
//!
//! Strict subset of OpenAPI 3.x; same shape as smartdoc output for
//! diff-compatibility against `jkit drift check --plan`.
//!
//! ## Required sections
//!
//! - `openapi:` — string starting with `3.` (e.g. `3.0.3`, `3.1.0`)
//! - `info:` — mapping with `title` and `version`
//! - At least one of `paths` (added/modified/removed paths) or
//!   `components.schemas` (added/modified schemas) must be non-empty.
//!   A file with both empty is ambiguous: either omit the file (no API
//!   change) or fill in the relevant section.
//!
//! ## Optional sections
//!
//! - `components.schemas:` — mapping of schema name → schema definition
//! - `components.securitySchemes:` — mapping (only when introducing a new
//!   auth scheme)
//! - Per-operation `security:` — list of `{<scheme>: [<scope-or-permission>]}`
//!
//! ## Lifecycle
//!
//! Written once by writing-plans; never mutated; archived with the run.
//! Absent when the change doesn't touch the public API — `validate-proposal`
//! and `drift check --plan` both treat absence as `{ok: true, absent: true}`.

pub mod validate;
