//! Argument parsing only. Nothing here decides anything.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "unsilo",
    version,
    about = "One view of your Claude Code conversations, whatever account you are signed in with"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// JSON on stdout, for piping.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
// Filters is wide by design: it is the shared selection surface, and boxing it
// would buy nothing in a type parsed once per process.
#[allow(clippy::large_enum_variant)]
pub enum Command {
    /// Read-only diagnosis: what exists, what cannot be seen, and whether writing is safe.
    Doctor {
        /// Exit 1 on any warning. For CI or a startup hook.
        #[arg(long)]
        strict: bool,
    },
    /// Search conversations. With no arguments, lists them all.
    Find {
        /// Full text search over title and first prompt.
        query: Option<String>,

        #[command(flatten)]
        filters: Filters,

        #[arg(long, value_enum, default_value_t = Format::Table)]
        format: Format,
    },
    /// Capture the current state, so there is a way back.
    Snapshot {
        /// claude: transcripts and desktop entries. store: unsilo's own state.
        #[arg(value_enum)]
        scope: SnapshotScope,
        /// Snapshot name. No slashes, no dots.
        #[arg(long)]
        name: String,
        /// Keep only the manifest of hashes, without the file bodies.
        #[arg(long)]
        metadata_only: bool,
    },
    /// Make the selected conversations visible under the active account.
    Apply {
        #[command(flatten)]
        filters: Filters,
        /// Print the plan without writing. Exits 4 when changes are pending.
        #[arg(long)]
        dry_run: bool,
        /// Keep the MCP fields, which belong to the account the session came from.
        #[arg(long)]
        keep_mcp: bool,
        /// Leave in place what an earlier apply projected but the filter no longer selects.
        #[arg(long)]
        no_prune: bool,
    },
    /// Turn unsilo off: remove what it projected, leave the store untouched.
    Off {
        #[arg(long)]
        dry_run: bool,
        /// Also delete the store and the index. Destructive.
        #[arg(long)]
        purge: bool,
    },
    /// Name an account or organization by hand, or capture the active one.
    ///
    /// Learning only works for the account signed in at the time, so an account
    /// that is never active again can only be named here.
    Label {
        /// Uuid, or enough of its start to be unambiguous. Accounts and
        /// organizations are both accepted; which one it is comes from the id.
        id: Option<String>,
        /// The email for an account, or the display name for an organization.
        name: Option<String>,
        /// Capture whatever account is signed in right now, and nothing else.
        /// Writes only inside the store, which makes it safe in a startup hook.
        #[arg(long)]
        learn: bool,
        /// Show what is known, and where each label came from.
        #[arg(long)]
        list: bool,
    },
    /// Put back what a snapshot captured.
    Restore {
        /// Snapshot name, or a path to a .tar.zst.
        name: String,
        /// Print the plan without writing. Exits 4 when changes are pending.
        #[arg(long)]
        dry_run: bool,
        /// Overwrite files that diverge from the snapshot.
        #[arg(long)]
        force: bool,
        /// Skip the ones that diverge instead of stopping.
        #[arg(long)]
        skip_conflicts: bool,
        /// Rewrite a path prefix: OLD=NEW. Repeatable.
        #[arg(long = "rewrite-cwd")]
        rewrite_cwd: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SnapshotScope {
    Claude,
    Store,
}

impl From<SnapshotScope> for crate::snapshot::Scope {
    fn from(value: SnapshotScope) -> Self {
        match value {
            SnapshotScope::Claude => crate::snapshot::Scope::Claude,
            SnapshotScope::Store => crate::snapshot::Scope::Store,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Table,
    Json,
    /// One path per line, for piping.
    Paths,
    /// A ready to run command that reopens the session.
    Resume,
}

/// The same filters for `find` and `apply`. One type, so previewing with one and
/// applying with the other cannot drift apart.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct Filters {
    /// Account by email. Repeatable.
    #[arg(long = "email")]
    pub emails: Vec<String>,
    /// Account by uuid, for accounts whose email is not resolved. Repeatable.
    #[arg(long = "account")]
    pub accounts: Vec<String>,
    /// Organization by uuid. Repeatable.
    #[arg(long = "org")]
    pub orgs: Vec<String>,
    /// Prefix of the recorded cwd.
    #[arg(long)]
    pub cwd: Option<String>,
    /// Project directory name.
    #[arg(long)]
    pub project: Option<String>,
    /// Recorded git branch.
    #[arg(long)]
    pub branch: Option<String>,
    /// Model recorded for the session, matched as a substring.
    #[arg(long)]
    pub model: Option<String>,
    /// Substring of the title, or of the first prompt when there is no title.
    #[arg(long)]
    pub title: Option<String>,
    /// Session uuid prefix.
    #[arg(long)]
    pub id: Option<String>,
    /// ISO8601 or relative: 7d, 3w, 6mo, 1y.
    #[arg(long)]
    pub since: Option<String>,
    /// Upper bound on last activity, same forms as --since.
    #[arg(long)]
    pub until: Option<String>,
    /// code or cowork. Repeatable.
    #[arg(long = "surface")]
    pub surfaces: Vec<String>,
    /// Only sessions the desktop has archived.
    #[arg(long)]
    pub archived: bool,
    /// Include the ones with a tombstone.
    #[arg(long)]
    pub include_deleted: bool,
    /// Include sidechains, team sessions and daemon sessions.
    #[arg(long)]
    pub include_hidden: bool,
    /// Show at most this many. Matches are counted before it applies.
    #[arg(long)]
    pub limit: Option<usize>,
    /// recent, created or size.
    #[arg(long, default_value = "recent")]
    pub sort: String,
}

impl Filters {
    pub fn to_filter(&self) -> crate::Result<crate::filter::Filter> {
        use crate::claude::desktop::Surface;
        let surfaces = self
            .surfaces
            .iter()
            .map(|s| match s.as_str() {
                "code" => Ok(Surface::Code),
                "cowork" => Ok(Surface::Cowork),
                other => Err(crate::Error::Usage(format!(
                    "unknown surface {other:?}, use code or cowork"
                ))),
            })
            .collect::<crate::Result<Vec<_>>>()?;
        let sort = crate::filter::Sort::parse(&self.sort).ok_or_else(|| {
            crate::Error::Usage(format!(
                "unknown sort order {:?}, use recent, created or size",
                self.sort
            ))
        })?;
        Ok(crate::filter::Filter {
            emails: self.emails.clone(),
            accounts: self.accounts.clone(),
            orgs: self.orgs.clone(),
            cwd: self.cwd.clone(),
            project: self.project.clone(),
            branch: self.branch.clone(),
            model: self.model.clone(),
            title: self.title.clone(),
            id: self.id.clone(),
            since: self.since.clone(),
            until: self.until.clone(),
            surfaces,
            archived_only: self.archived,
            include_deleted: self.include_deleted,
            include_hidden: self.include_hidden,
            query: None,
            limit: self.limit,
            sort,
        })
    }
}
