//! Search and listing. With no query and no filters it is the full list, which
//! is why there is no separate `list` command.

use crate::claude::identity::Identities;
use crate::env::Env;
use crate::error::Result;
use crate::filter::Filter;
use crate::index::{Index, Row};
use crate::ops::ingest;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Results {
    pub schema: u32,
    pub rows: Vec<Row>,
    /// How many matched before `--limit` was applied.
    pub matched: usize,
    pub total: usize,
    pub identities: IdentityLookup,
}

/// Enough of the identity map to render account columns without a second pass.
#[derive(Debug, Clone, Default, Serialize)]
pub struct IdentityLookup {
    pub emails: std::collections::BTreeMap<String, String>,
    pub orgs: std::collections::BTreeMap<String, String>,
}

impl From<&Identities> for IdentityLookup {
    fn from(ids: &Identities) -> Self {
        Self {
            emails: ids.accounts.iter().map(|(k, v)| (k.clone(), v.name.clone())).collect(),
            orgs: ids.orgs.iter().map(|(k, v)| (k.clone(), v.name.clone())).collect(),
        }
    }
}

pub fn run(env: &Env, filter: &Filter) -> Result<Results> {
    let index = Index::open(&env.index_path())?;
    ingest::run(env, &index)?;
    let identities = ingest::identities(env)?;
    let resolved = filter.resolve(&identities, env.clock.now_ms())?;

    // Count first, then page: "3 de 132" needs the number before the limit.
    let unlimited = crate::filter::Resolved { limit: None, ..resolved.clone() };
    let matched = index.query(&unlimited)?.len();
    let rows = index.query(&resolved)?;

    Ok(Results {
        schema: 1,
        rows,
        matched,
        total: index.count_sessions()?,
        identities: IdentityLookup::from(&identities),
    })
}
