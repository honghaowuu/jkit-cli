pub mod check;
pub mod report;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum DriftCmd {
    /// Schema-validate `<run>/proposed-api.yaml` against the strict OpenAPI
    /// 3.x subset writing-plans is required to produce. Absent file is
    /// treated as "this run doesn't touch the public API" and validates clean.
    ValidateProposal {
        /// Path to a `.jkit/<run>/` directory.
        #[arg(long)]
        run: PathBuf,
    },
    /// Diff smartdoc(code) against either the run's `proposed-api.yaml`
    /// (`--plan` mode) or a published `contract.yaml` (`--against
    /// published-contract` mode). Exactly one of `--plan` / `--against`
    /// must be passed.
    ///
    /// Diff scope (both modes): paths, methods, response status codes,
    /// request body required fields, parameters, response body required
    /// fields. Schema-walking resolves `$ref` to `components.schemas` and
    /// recurses into `allOf`. Type-level diffs and `oneOf`/`anyOf` walks
    /// are not emitted.
    ///
    /// `--against published-contract` annotates each finding with
    /// `severity` (`issue` for major-bump-required removals/tightenings;
    /// `warning` for minor-bump additions/loosenings).
    Check {
        /// Path to `<run>/plan.md` — the run dir is its parent. Triggers
        /// `--plan` mode. Mutually exclusive with `--against`.
        #[arg(long, conflicts_with = "against")]
        plan: Option<PathBuf>,
        /// Currently the only valid value is `published-contract`. The
        /// reference document is read from `--contract-yaml`. Auto-resolution
        /// from `.jkit/contract.json` lands in a follow-up slice.
        #[arg(long, value_parser = ["published-contract"])]
        against: Option<String>,
        /// Path to the published `contract.yaml` to diff against. Required
        /// when `--against published-contract` is set.
        #[arg(long = "contract-yaml")]
        contract_yaml: Option<PathBuf>,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
    },
    /// Report bi-directional drift between controllers and api-spec.yaml.
    /// Code-without-spec entries route to /write-change → /spec-delta;
    /// spec-without-code entries are either unimplemented endpoints or
    /// stale spec — the human decides via the same route.
    ///
    /// **Deprecated:** retargeted as `drift check --against published-contract`
    /// in a later slice. Kept available for legacy projects until then.
    Report {
        /// Restrict to a single domain (matches the resolved slug from
        /// `--domain-map` if present, else `derive_domain_slug`). Omit to
        /// report drift across every detected domain.
        domain: Option<String>,
        #[arg(long, default_value = "src/main/java")]
        src: PathBuf,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
        #[arg(long, default_value = "docs/domains")]
        domains_root: PathBuf,
        /// Optional `.jkit/migration-domain-map.json`. When set, controllers
        /// are bucketed by the resolved slug from the file; otherwise each
        /// controller's `derive_domain_slug` (Controller-class-name based)
        /// is used. Match scaffold-docs with this flag whenever the project
        /// has run `infer-domains` + a domain-map review.
        #[arg(long)]
        domain_map: Option<PathBuf>,
    },
}

pub fn run(cmd: DriftCmd) -> Result<()> {
    match cmd {
        DriftCmd::ValidateProposal { run } => crate::proposed_api::validate::run(&run),
        DriftCmd::Check {
            plan,
            against,
            contract_yaml,
            pom,
        } => match (plan, against, contract_yaml) {
            (Some(plan_path), None, None) => check::run(&plan_path, &pom),
            (None, Some(_), Some(contract_path)) => {
                check::run_against_contract(&contract_path, &pom)
            }
            (None, Some(_), None) => Err(anyhow::anyhow!(
                "--against published-contract requires --contract-yaml <path>"
            )),
            (None, None, _) => Err(anyhow::anyhow!(
                "exactly one of --plan or --against must be supplied"
            )),
            (Some(_), Some(_), _) => Err(anyhow::anyhow!(
                "--plan and --against are mutually exclusive"
            )),
            (Some(_), None, Some(_)) => Err(anyhow::anyhow!(
                "--contract-yaml requires --against published-contract"
            )),
        },
        DriftCmd::Report {
            domain,
            src,
            pom,
            domains_root,
            domain_map,
        } => report::run(
            domain.as_deref(),
            &src,
            &pom,
            &domains_root,
            domain_map.as_deref(),
        ),
    }
}
