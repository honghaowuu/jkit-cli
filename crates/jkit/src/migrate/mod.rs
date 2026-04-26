pub mod call_graph;
pub mod infer_domains;
pub mod parse;
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
    ///
    /// Optional content-quality flags:
    /// - `--use-smartdoc`: smart-doc:openapi for authoritative api-spec.yaml
    /// - `--use-call-graph`: tree-sitter-java for controller→service call-graph
    ///   skeleton in api-implement-logic.md (rules stay TODO; structure
    ///   comes from code).
    /// Each flag falls back to regex/skeleton output independently if its
    /// underlying tool fails.
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
        /// Build a controller→service call graph via tree-sitter-java and
        /// render it into api-implement-logic.md (rules stay TODO; structure
        /// comes from code, 2 levels deep, simple-name resolution).
        #[arg(long)]
        use_call_graph: bool,
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
            use_call_graph,
        } => scaffold_docs::run(
            &src,
            &pom,
            &out,
            domain_map.as_deref(),
            use_smartdoc,
            use_call_graph,
        ),
    }
}
