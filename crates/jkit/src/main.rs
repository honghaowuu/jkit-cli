use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(name = "jkit", version, about = "Java-specific developer pipeline CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Top,
}

#[derive(Subcommand, Debug)]
enum Top {
    Pom {
        #[command(subcommand)]
        cmd: jkit::pom::PomCmd,
    },
    Coverage(jkit::coverage::CoverageArgs),
    Scenarios {
        #[command(subcommand)]
        cmd: jkit::scenarios::ScenariosCmd,
    },
    Drift {
        #[command(subcommand)]
        cmd: jkit::drift::DriftCmd,
    },
    Migrate {
        #[command(subcommand)]
        cmd: jkit::migrate::MigrateCmd,
    },
    Migration {
        #[command(subcommand)]
        cmd: jkit::migration::MigrationCmd,
    },
    Contract {
        #[command(subcommand)]
        cmd: jkit::contract::ContractCmd,
    },
    Changes {
        #[command(subcommand)]
        cmd: jkit::changes::ChangesCmd,
    },
    Standards {
        #[command(subcommand)]
        cmd: jkit::standards::StandardsCmd,
    },
    Domains {
        #[command(subcommand)]
        cmd: jkit::domains::DomainsCmd,
    },
    /// Aggregate every `@Permission` code referenced under src — used
    /// when authoring new auth-protected endpoints to pick a non-colliding
    /// `<module>.<OPERATION>` code.
    Permissions {
        #[command(subcommand)]
        cmd: jkit::permissions::PermissionsCmd,
    },
    /// Compact JSON view of one domain (slug + description + use_when +
    /// scenario summary). Used by spec-delta clarification, writing-plans,
    /// and java-tdd to gather per-domain context without per-file reads.
    #[command(name = "domain-context")]
    DomainContext(jkit::domain_context::DomainContextArgs),
    /// On-demand OpenAPI 3.x slice for one domain — runs smartdoc against
    /// the project's controllers and filters to the requested slug.
    #[command(name = "api-spec")]
    ApiSpec {
        #[command(subcommand)]
        cmd: jkit::api_spec::ApiSpecCmd,
    },
    /// One-shot project bootstrap: changes bootstrap + standards init + scaffold.
    /// Idempotent — re-running completes only what's missing.
    Init {
        #[command(subcommand)]
        cmd: Option<jkit::init::InitCmd>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Top::Pom { cmd } => jkit::pom::run(cmd),
        Top::Coverage(args) => jkit::coverage::run(args),
        Top::Scenarios { cmd } => jkit::scenarios::run(cmd),
        Top::Drift { cmd } => jkit::drift::run(cmd),
        Top::Migrate { cmd } => jkit::migrate::run(cmd),
        Top::Migration { cmd } => jkit::migration::run(cmd),
        Top::Contract { cmd } => jkit::contract::run(cmd),
        Top::Changes { cmd } => jkit::changes::run(cmd),
        Top::Standards { cmd } => jkit::standards::run(cmd),
        Top::Domains { cmd } => jkit::domains::run(cmd),
        Top::Permissions { cmd } => jkit::permissions::run(cmd),
        Top::DomainContext(args) => jkit::domain_context::run(args),
        Top::ApiSpec { cmd } => jkit::api_spec::run(cmd),
        Top::Init { cmd } => match cmd {
            Some(c) => jkit::init::run(c),
            None => jkit::init::run_umbrella(),
        },
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => jkit::envelope::print_err(&format!("{e:#}"), None),
    }
}
