//! Putting a name on an account or organization by hand.
//!
//! Learning only ever works for the account signed in at the time, so an
//! account that is never active again can never be labelled automatically.
//! Editing `identities.json` was the only way out, which is not a way out.

use crate::claude::desktop;
use crate::claude::identity::{self, Identities, Source};
use crate::env::Env;
use crate::error::{Error, Result};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Account,
    Org,
}

#[derive(Debug, Clone, Serialize)]
pub struct Labelled {
    pub kind: Kind,
    pub uuid: String,
    pub name: String,
    pub replaced: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Learned {
    /// Labels captured that were not known before.
    pub added: usize,
    pub active_account: Option<String>,
    pub active_email: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Listing {
    pub accounts: Vec<Row>,
    pub orgs: Vec<Row>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Row {
    pub uuid: String,
    pub name: Option<String>,
    pub source: Option<Source>,
    pub sessions: usize,
    pub is_active: bool,
}

/// Captures whatever the active account is right now and nothing else.
///
/// This is the safe target for a session-start hook: it writes only inside the
/// store, so it cannot surprise anyone who wires it up.
pub fn learn(env: &Env) -> Result<Learned> {
    let path = env.unsilo_home.join("identities.json");
    let mut identities = Identities::load(&path)?;
    let added = identities.learn_from(&env.home);
    if added > 0 {
        identities.save(&path)?;
    }
    let active = identity::active(&env.home);
    Ok(Learned {
        added,
        active_email: active.as_ref().and_then(|a| a.email.clone()),
        active_account: active.map(|a| a.account),
    })
}

pub fn set(env: &Env, id: &str, name: &str) -> Result<Labelled> {
    if name.trim().is_empty() {
        return Err(Error::Usage("the label cannot be empty".to_owned()));
    }
    let (accounts, orgs) = known_uuids(env);
    let kind = resolve_kind(id, &accounts, &orgs)?;
    let uuid = match kind {
        Kind::Account => unique_match(id, &accounts, "account")?,
        Kind::Org => unique_match(id, &orgs, "organization")?,
    };

    let path = env.unsilo_home.join("identities.json");
    let mut identities = Identities::load(&path)?;
    let replaced = match kind {
        Kind::Account => identities.email(&uuid).map(ToOwned::to_owned),
        Kind::Org => identities.org_name(&uuid).map(ToOwned::to_owned),
    };
    match kind {
        Kind::Account => identities.set_manual_account(&uuid, name),
        Kind::Org => identities.set_manual_org(&uuid, name),
    }
    identities.save(&path)?;

    Ok(Labelled { kind, uuid, name: name.to_owned(), replaced })
}

pub fn list(env: &Env) -> Result<Listing> {
    let path = env.unsilo_home.join("identities.json");
    let mut identities = Identities::load(&path)?;
    identities.learn_from(&env.home);
    let active = identity::active(&env.home);

    let mut accounts: Vec<Row> = Vec::new();
    let mut orgs: Vec<Row> = Vec::new();
    for root in &env.user_data {
        let inventory = desktop::inventory(root);
        for (account, org_list) in &inventory.scopes {
            if accounts.iter().any(|r| &r.uuid == account) {
                continue;
            }
            accounts.push(Row {
                uuid: account.clone(),
                name: identities.email(account).map(ToOwned::to_owned),
                source: identities.accounts.get(account).map(|l| l.source),
                sessions: inventory.entries.iter().filter(|e| &e.scope.account == account).count(),
                is_active: active.as_ref().is_some_and(|a| &a.account == account),
            });
            for org in org_list {
                if orgs.iter().any(|r| &r.uuid == org) {
                    continue;
                }
                orgs.push(Row {
                    uuid: org.clone(),
                    name: identities.org_name(org).map(ToOwned::to_owned),
                    source: identities.orgs.get(org).map(|l| l.source),
                    sessions: inventory.entries.iter().filter(|e| &e.scope.org == org).count(),
                    is_active: active.as_ref().is_some_and(|a| &a.org == org),
                });
            }
        }
    }
    accounts.sort_by(|a, b| a.uuid.cmp(&b.uuid));
    orgs.sort_by(|a, b| a.uuid.cmp(&b.uuid));
    Ok(Listing { accounts, orgs })
}

fn known_uuids(env: &Env) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut accounts = BTreeSet::new();
    let mut orgs = BTreeSet::new();
    for root in &env.user_data {
        let inventory = desktop::inventory(root);
        for (account, org_list) in inventory.scopes {
            accounts.insert(account);
            orgs.extend(org_list);
        }
    }
    (accounts, orgs)
}

/// An account and an organization uuid look identical, so the kind comes from
/// which set the id belongs to rather than from a flag the user has to know.
fn resolve_kind(id: &str, accounts: &BTreeSet<String>, orgs: &BTreeSet<String>) -> Result<Kind> {
    let in_accounts = accounts.iter().any(|a| a.starts_with(id));
    let in_orgs = orgs.iter().any(|o| o.starts_with(id));
    match (in_accounts, in_orgs) {
        (true, false) => Ok(Kind::Account),
        (false, true) => Ok(Kind::Org),
        (true, true) => Err(Error::Usage(format!(
            "{id} matches both an account and an organization; use more characters"
        ))),
        (false, false) => Err(Error::Usage(format!(
            "no known account or organization starts with {id}. \
             run `unsilo label --list` to see them"
        ))),
    }
}

fn unique_match(id: &str, candidates: &BTreeSet<String>, what: &str) -> Result<String> {
    let matches: Vec<&String> = candidates.iter().filter(|c| c.starts_with(id)).collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(Error::Usage(format!("no known {what} starts with {id}"))),
        many => {
            Err(Error::Usage(format!("{id} matches {} {what}s; use more characters", many.len())))
        }
    }
}
