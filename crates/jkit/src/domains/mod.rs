//! `jkit domains` — manage the `docs/domains.yaml` index that names each
//! domain with a one-line `description` (entity/behavior framing) and
//! `use_when` (consumer-perspective task framing). The file is the routing
//! source for spec-delta and the metadata source for `publish-contract`.

pub mod add;
pub mod doctor;
pub mod list;
pub mod set;
pub mod yaml;

use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum DomainsCmd {
    /// List domains with their description and use_when.
    List,
    /// Add a domain. Idempotent when the existing entry has matching fields.
    Add {
        #[arg(long)]
        slug: String,
        #[arg(long)]
        description: String,
        #[arg(long = "use-when")]
        use_when: String,
    },
    /// Update a single domain's `description`.
    SetDescription {
        #[arg(long)]
        slug: String,
        #[arg(long)]
        text: String,
    },
    /// Update a single domain's `use_when`.
    SetUseWhen {
        #[arg(long)]
        slug: String,
        #[arg(long)]
        text: String,
    },
    /// Consistency checks over docs/domains.yaml.
    Doctor,
}

pub fn run(cmd: DomainsCmd) -> Result<()> {
    match cmd {
        DomainsCmd::List => list::run(),
        DomainsCmd::Add {
            slug,
            description,
            use_when,
        } => add::run(&slug, &description, &use_when),
        DomainsCmd::SetDescription { slug, text } => set::run_description(&slug, &text),
        DomainsCmd::SetUseWhen { slug, text } => set::run_use_when(&slug, &text),
        DomainsCmd::Doctor => doctor::run(),
    }
}
