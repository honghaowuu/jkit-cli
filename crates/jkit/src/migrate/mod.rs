pub mod infer_domains;
pub mod scaffold_docs;
pub mod smartdoc;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum MigrateCmd {
    /// Detect controllers and emit candidate `(slug, [controllers])` groupings
    /// for human review. Output is consumed by `/migrate-project` to build
    /// `.jkit/migration-domain-map.json`, the authoritative mapping that drives
    /// every downstream backfill step. Read-only.
    InferDomains {
        #[arg(long, default_value = "src/main/java")]
        src: PathBuf,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
    },
    /// Reverse-engineer skeleton docs/domains/<n>/ files from existing controllers.
    /// Idempotent: writes only files that don't already exist; surfaces
    /// already-present files in the JSON output. Controllers are grouped by the
    /// resolved domain map (`--domain-map`) when provided; otherwise falls back
    /// to the same `derive_domain_slug` rule the contract pipeline uses.
    /// With `--use-smartdoc`, runs smart-doc:openapi and slices the resulting
    /// OpenAPI doc per domain — produces authoritative request/response schemas
    /// from DTOs instead of regex-derived skeletons. Falls back to regex output
    /// for any domain smartdoc fails to cover.
    ScaffoldDocs {
        #[arg(long, default_value = "src/main/java")]
        src: PathBuf,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
        #[arg(long, default_value = "docs/domains")]
        out: PathBuf,
        /// Path to `.jkit/migration-domain-map.json` produced by `infer-domains`
        /// + human review. When set, controller→slug mapping comes from the file
        /// instead of `derive_domain_slug`.
        #[arg(long)]
        domain_map: Option<PathBuf>,
        /// Run smart-doc:openapi to produce authoritative api-spec.yaml per
        /// domain (request/response schemas from DTOs). Requires Maven and a
        /// working build. Falls back to regex output if smartdoc fails.
        #[arg(long)]
        use_smartdoc: bool,
    },
}

pub fn run(cmd: MigrateCmd) -> Result<()> {
    match cmd {
        MigrateCmd::InferDomains { src, pom } => infer_domains::run(&src, &pom),
        MigrateCmd::ScaffoldDocs {
            src,
            pom,
            out,
            domain_map,
            use_smartdoc,
        } => scaffold_docs::run(&src, &pom, &out, domain_map.as_deref(), use_smartdoc),
    }
}
