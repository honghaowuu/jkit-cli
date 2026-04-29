pub mod check;

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
        /// reference document is auto-resolved from `.jkit/contract.json`
        /// (`contractRepo` URL → shallow git clone → `reference/contract.yaml`)
        /// unless `--contract-yaml <path>` is given.
        #[arg(long, value_parser = ["published-contract"])]
        against: Option<String>,
        /// Path to a local `contract.yaml`. When set, skips the
        /// `.jkit/contract.json` resolution and shallow clone — useful
        /// for tests, CI fixtures, or when the contract repo is already
        /// checked out locally.
        #[arg(long = "contract-yaml")]
        contract_yaml: Option<PathBuf>,
        #[arg(long, default_value = "pom.xml")]
        pom: PathBuf,
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
                check::run_against_contract_from_file(&contract_path, &pom)
            }
            (None, Some(_), None) => check::run_against_contract_from_repo(&pom),
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
    }
}
