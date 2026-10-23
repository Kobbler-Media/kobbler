//! Follow Hardcover's view of the authors, series and books we link to it.
//!
//! Hardcover merges duplicates (setting `canonical_id` on the loser) and deletes
//! entries outright. Its author and series queries filter merged rows out, so without
//! this a merged or deleted entity just stops syncing and lingers here: a series whose
//! enrich fails forever, discovered books that point at nothing.
//!
//! The rule: where Hardcover merged, merge; where it deleted, delete. Anything holding
//! user data (files, wants, reading state -- see `book_has_user_data`) is never
//! deleted: it is unlinked and flagged in `provider_link_issues` instead, and two
//! books that both hold user data are never merged automatically.

use std::collections::{HashMap, HashSet};

use sqlx::PgPool;
use tracing::{error, info, warn};
use uuid::Uuid;

use super::hardcover::{HardcoverAuthorProvider, HardcoverEntity};
use crate::repositories::provider_reconcile::{self as repo, EntityKind, IssueKind};
use crate::repositories::{Result, authors, book_series, books, series};

pub const PROVIDER: &str = "hardcover";

/// Longest `canonical_id` chain followed. Longer chains (or cycles) are left alone.
const MAX_HOPS: usize = 5;

/// A pass that finds more deletions than this (and more than a tenth of the rows) is
/// far more likely a broken lookup than Hardcover deleting that much, so its
/// deletions are withheld. Merges still apply.
const DELETION_FLOOR: usize = 25;

/// What one lookup said about an id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Looked {
    Canonical,
    Merged(i64),
    Missing,
}

/// Where Hardcover keeps an id now, after following merges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remote {
    Active,
    MergedInto(i64),
    Deleted,
}

/// Follow `id` through `known`. `Err(next)` names the id that still needs looking up.
/// An id merged into one Hardcover has since deleted counts as deleted.
pub fn resolve(known: &HashMap<i64, Looked>, id: i64) -> std::result::Result<Remote, i64> {
    let mut cur = id;
    for _ in 0..=MAX_HOPS {
        match known.get(&cur) {
            None => return Err(cur),
            Some(Looked::Missing) => return Ok(Remote::Deleted),
            Some(Looked::Canonical) if cur == id => return Ok(Remote::Active),
            Some(Looked::Canonical) => return Ok(Remote::MergedInto(cur)),
            Some(Looked::Merged(next)) => cur = *next,
        }
    }
    Ok(Remote::Active)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Keep,
    Delete,
    /// Point at the canonical id; nothing local carries it yet.
    Repoint(i64),
    /// Fold into the local series/author that already carries the canonical id.
    MergeInto(Uuid),
    /// The local book carrying the canonical id holds nothing, this one does: drop
    /// that one and let this one take the canonical id.
    ReplaceTarget {
        target: Uuid,
        canonical: i64,
    },
    Unlink,
    /// Both this book and the one carrying the canonical id hold user data.
    FlagConflict {
        target: Uuid,
        canonical: i64,
    },
}

/// `target` is the other local row carrying the canonical id, and whether it holds
/// user data (only consulted for books).
pub fn decide(
    kind: EntityKind,
    remote: Remote,
    has_user_data: bool,
    target: Option<(Uuid, bool)>,
) -> Action {
    match remote {
        Remote::Active => Action::Keep,
        Remote::Deleted if has_user_data => Action::Unlink,
        Remote::Deleted => Action::Delete,
        Remote::MergedInto(canonical) => match (kind, target) {
            (EntityKind::Book, Some((target, target_has_user_data))) => {
                if !has_user_data {
                    Action::Delete
                } else if !target_has_user_data {
                    Action::ReplaceTarget { target, canonical }
                } else {
                    Action::FlagConflict { target, canonical }
                }
            }
            // Its canonical edition arrives through the author/series sync.
            (EntityKind::Book, None) if !has_user_data => Action::Delete,
            (EntityKind::Book, None) => Action::Repoint(canonical),
            (_, Some((target, _))) => Action::MergeInto(target),
            (_, None) => Action::Repoint(canonical),
        },
    }
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Report {
    pub checked: usize,
    pub deleted: usize,
    pub merged: usize,
    pub repointed: usize,
    pub unlinked: usize,
    pub conflicts: usize,
    pub books_removed: usize,
    /// Deletions skipped because the pass found implausibly many.
    pub deletions_withheld: usize,
}

fn entity(kind: EntityKind) -> HardcoverEntity {
    match kind {
        EntityKind::Author => HardcoverEntity::Author,
        EntityKind::Series => HardcoverEntity::Series,
        EntityKind::Book => HardcoverEntity::Book,
    }
}

/// Look up `ids` and every id they were merged into, until each resolves.
async fn look_up(
    hc: &HardcoverAuthorProvider,
    kind: EntityKind,
    ids: &[i64],
) -> std::result::Result<HashMap<i64, Looked>, String> {
    let mut known = HashMap::with_capacity(ids.len());
    let mut pending: Vec<i64> = ids.to_vec();
    for _ in 0..=MAX_HOPS {
        pending.sort_unstable();
        pending.dedup();
        if pending.is_empty() {
            break;
        }
        let found = hc.lookup_canonical(entity(kind), &pending).await?;
        for id in &pending {
            let looked = match found.get(id) {
                None => Looked::Missing,
                Some(None) => Looked::Canonical,
                Some(Some(c)) if c == id => Looked::Canonical,
                Some(Some(c)) => Looked::Merged(*c),
            };
            known.insert(*id, looked);
        }
        pending = ids
            .iter()
            .filter_map(|id| resolve(&known, *id).err())
            .collect();
    }
    Ok(known)
}

/// Reconcile every author, series and book linked to Hardcover. Authors go first so
/// their books move before series and books are judged.
pub async fn run(
    pool: &PgPool,
    nats: &nats_jobs::Publisher,
    hc: &HardcoverAuthorProvider,
) -> std::result::Result<Report, String> {
    let mut report = Report::default();
    for kind in [EntityKind::Author, EntityKind::Series, EntityKind::Book] {
        let rows = repo::list_linked(pool, kind, PROVIDER)
            .await
            .map_err(|e| e.to_string())?;
        reconcile_rows(pool, nats, hc, kind, &rows, &mut report).await?;
    }
    if let Ok(pruned) = repo::prune_flags(pool).await
        && pruned > 0
    {
        info!(
            pruned,
            "Dropped link issues for entities that no longer exist"
        );
    }
    Ok(report)
}

/// Reconcile one entity and return what Hardcover says about it. Sync jobs call this
/// when Hardcover stops returning something, so a merge or deletion is applied at once
/// instead of failing until the nightly pass.
pub async fn reconcile_entity(
    pool: &PgPool,
    nats: &nats_jobs::Publisher,
    hc: &HardcoverAuthorProvider,
    kind: EntityKind,
    id: Uuid,
    external_id: i64,
) -> std::result::Result<Remote, String> {
    let mut report = Report::default();
    let known = look_up(hc, kind, &[external_id]).await?;
    let remote = resolve(&known, external_id).unwrap_or(Remote::Active);
    apply(pool, nats, kind, id, external_id, remote, &mut report)
        .await
        .map_err(|e| e.to_string())?;
    Ok(remote)
}

/// For a sync job whose Hardcover fetch failed: if Hardcover merged or deleted the
/// entity, follow it and return `true` so the job can stop instead of retrying
/// something that will never come back. `false` means it is still there (or Hardcover
/// could not be asked), so the failure is a real one.
pub async fn settle_after_fetch_failure(
    pool: &PgPool,
    nats: &nats_jobs::Publisher,
    api_key: &str,
    kind: EntityKind,
    id: Uuid,
    external_id: &str,
) -> bool {
    let Ok(ext) = external_id.parse::<i64>() else {
        return false;
    };
    let hc = HardcoverAuthorProvider::new(api_key.to_string());
    match reconcile_entity(pool, nats, &hc, kind, id, ext).await {
        Ok(Remote::Active) => false,
        Ok(remote) => {
            info!(entity = kind.as_str(), id = %id, external_id = ext, remote = ?remote, "Hardcover no longer has this; followed it");
            true
        }
        Err(e) => {
            warn!(entity = kind.as_str(), id = %id, error = %e, "Could not check against Hardcover");
            false
        }
    }
}

async fn reconcile_rows(
    pool: &PgPool,
    nats: &nats_jobs::Publisher,
    hc: &HardcoverAuthorProvider,
    kind: EntityKind,
    rows: &[(Uuid, i64)],
    report: &mut Report,
) -> std::result::Result<(), String> {
    let ids: Vec<i64> = rows
        .iter()
        .map(|(_, ext)| *ext)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let known = look_up(hc, kind, &ids).await?;

    let deletions = rows
        .iter()
        .filter(|(_, ext)| resolve(&known, *ext) == Ok(Remote::Deleted))
        .count();
    let withhold_deletions = deletions > DELETION_FLOOR.max(rows.len() / 10);
    if withhold_deletions {
        error!(
            entity = kind.as_str(),
            deletions,
            rows = rows.len(),
            "Hardcover reconcile found implausibly many deletions; withholding them"
        );
    }

    for &(id, ext) in rows {
        report.checked += 1;
        let Ok(remote) = resolve(&known, ext) else {
            continue;
        };
        if remote == Remote::Deleted && withhold_deletions {
            report.deletions_withheld += 1;
            continue;
        }
        if let Err(e) = apply(pool, nats, kind, id, ext, remote, report).await {
            warn!(entity = kind.as_str(), id = %id, external_id = ext, error = %e, "Hardcover reconcile failed for one entity");
        }
    }
    Ok(())
}

async fn apply(
    pool: &PgPool,
    nats: &nats_jobs::Publisher,
    kind: EntityKind,
    id: Uuid,
    ext: i64,
    remote: Remote,
    report: &mut Report,
) -> Result<()> {
    if remote == Remote::Active {
        return repo::clear_flag(pool, kind, id, PROVIDER).await;
    }
    // An earlier step of this run may already have removed it.
    if !repo::exists(pool, kind, id).await? {
        return Ok(());
    }

    let has_user_data = repo::has_user_data(pool, kind, id).await?;
    let target = match remote {
        Remote::MergedInto(canonical) => {
            match repo::find_linked(pool, kind, PROVIDER, canonical, id).await? {
                Some(t) if kind == EntityKind::Book => {
                    Some((t, repo::has_user_data(pool, kind, t).await?))
                }
                Some(t) => Some((t, false)),
                None => None,
            }
        }
        _ => None,
    };
    let action = decide(kind, remote, has_user_data, target);
    info!(entity = kind.as_str(), id = %id, external_id = ext, remote = ?remote, action = ?action, "Following Hardcover");

    match action {
        Action::Keep => {}
        Action::Delete => {
            report.deleted += 1;
            report.books_removed += delete(pool, nats, kind, id).await?;
        }
        Action::Repoint(canonical) => {
            repo::repoint(pool, kind, id, PROVIDER, canonical).await?;
            repo::clear_flag(pool, kind, id, PROVIDER).await?;
            report.repointed += 1;
        }
        Action::MergeInto(target) => {
            merge(pool, nats, kind, id, target).await?;
            report.merged += 1;
        }
        Action::ReplaceTarget { target, canonical } => {
            let mut tx = pool.begin().await?;
            let removed = repo::delete_books_without_user_data(&mut *tx, &[target]).await?;
            if removed.is_empty() {
                // It picked up user data since we looked; leave both for a person.
                tx.rollback().await?;
                repo::flag(
                    pool,
                    kind,
                    id,
                    PROVIDER,
                    ext,
                    IssueKind::MergeConflict,
                    Some(canonical),
                    Some(target),
                )
                .await?;
                report.conflicts += 1;
            } else {
                repo::repoint(&mut *tx, kind, id, PROVIDER, canonical).await?;
                repo::clear_flag(&mut *tx, kind, id, PROVIDER).await?;
                tx.commit().await?;
                crate::jobs::enqueue_book_index(pool, nats, target).await;
                report.merged += 1;
                report.books_removed += 1;
            }
        }
        Action::Unlink => {
            repo::unlink(pool, kind, id, PROVIDER).await?;
            repo::flag(
                pool,
                kind,
                id,
                PROVIDER,
                ext,
                IssueKind::Deleted,
                None,
                None,
            )
            .await?;
            report.unlinked += 1;
        }
        Action::FlagConflict { target, canonical } => {
            repo::flag(
                pool,
                kind,
                id,
                PROVIDER,
                ext,
                IssueKind::MergeConflict,
                Some(canonical),
                Some(target),
            )
            .await?;
            report.conflicts += 1;
        }
    }
    Ok(())
}

/// Delete an entity holding no user data, with the books only it brought in. Returns
/// how many books went with it.
async fn delete(
    pool: &PgPool,
    nats: &nats_jobs::Publisher,
    kind: EntityKind,
    id: Uuid,
) -> Result<usize> {
    let (affected, removed) = match kind {
        EntityKind::Book => {
            let removed = repo::delete_books_without_user_data(pool, &[id]).await?;
            (Vec::new(), removed)
        }
        EntityKind::Series => {
            let mut tx = pool.begin().await?;
            let members = books::get_ids_by_series(&mut *tx, id).await?;
            series::delete(&mut *tx, id).await?;
            let removed = repo::delete_unanchored_books(&mut *tx, &members).await?;
            tx.commit().await?;
            crate::jobs::enqueue_series_index(pool, nats, id).await;
            (members, removed)
        }
        EntityKind::Author => {
            let mut tx = pool.begin().await?;
            let linked = authors::get_book_ids(&mut *tx, id).await?;
            let removed = repo::delete_discovered_books_without_user_data(&mut *tx, id).await?;
            authors::delete(&mut *tx, id).await?;
            tx.commit().await?;
            crate::jobs::enqueue_author_index(pool, nats, id).await;
            (linked, removed)
        }
    };
    // Indexing a missing book removes it from the index; a surviving one is refreshed.
    let mut touched: HashSet<Uuid> = affected.into_iter().collect();
    touched.extend(removed.iter().copied());
    for book_id in touched {
        crate::jobs::enqueue_book_index(pool, nats, book_id).await;
    }
    Ok(removed.len())
}

/// Fold a series or author into the local one carrying its canonical id, the way the
/// manual merge does, keeping its monitoring if the survivor had none.
async fn merge(
    pool: &PgPool,
    nats: &nats_jobs::Publisher,
    kind: EntityKind,
    from: Uuid,
    to: Uuid,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let affected = match kind {
        EntityKind::Book => return Ok(()),
        EntityKind::Series => {
            let members = books::get_ids_by_series(&mut *tx, from).await?;
            books::move_books_to_series(&mut *tx, from, to).await?;
            repo::carry_monitoring(&mut *tx, kind, from, to).await?;
            series::delete(&mut *tx, from).await?;
            for &book_id in &members {
                book_series::auto_select_primary(&mut *tx, book_id, false).await?;
            }
            members
        }
        EntityKind::Author => {
            authors::copy_book_links(&mut *tx, from, to).await?;
            authors::unlink_all_books(&mut *tx, from).await?;
            repo::move_discovered_books(&mut *tx, from, to).await?;
            repo::carry_monitoring(&mut *tx, kind, from, to).await?;
            authors::delete(&mut *tx, from).await?;
            authors::get_book_ids(&mut *tx, to).await?
        }
    };
    tx.commit().await?;

    match kind {
        EntityKind::Series => {
            crate::jobs::enqueue_series_index(pool, nats, from).await;
            crate::jobs::enqueue_series_index(pool, nats, to).await;
        }
        EntityKind::Author => {
            crate::jobs::enqueue_author_index(pool, nats, from).await;
            crate::jobs::enqueue_author_index(pool, nats, to).await;
        }
        EntityKind::Book => {}
    }
    for book_id in affected {
        crate::jobs::enqueue_book_index(pool, nats, book_id).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(entries: &[(i64, Looked)]) -> HashMap<i64, Looked> {
        entries.iter().copied().collect()
    }

    #[test]
    fn resolve_follows_merges_to_the_canonical_id() {
        let k = known(&[
            (1, Looked::Merged(2)),
            (2, Looked::Merged(3)),
            (3, Looked::Canonical),
        ]);
        assert_eq!(resolve(&k, 1), Ok(Remote::MergedInto(3)));
        assert_eq!(resolve(&k, 3), Ok(Remote::Active));
    }

    #[test]
    fn resolve_asks_for_the_next_hop_it_has_not_seen() {
        let k = known(&[(1, Looked::Merged(2))]);
        assert_eq!(resolve(&k, 1), Err(2));
    }

    #[test]
    fn merged_into_a_deleted_id_counts_as_deleted() {
        let k = known(&[(1, Looked::Merged(2)), (2, Looked::Missing)]);
        assert_eq!(resolve(&k, 1), Ok(Remote::Deleted));
    }

    #[test]
    fn a_cycle_is_left_alone() {
        let k = known(&[(1, Looked::Merged(2)), (2, Looked::Merged(1))]);
        assert_eq!(resolve(&k, 1), Ok(Remote::Active));
    }

    #[test]
    fn deletion_unlinks_anything_holding_user_data_and_deletes_the_rest() {
        for kind in [EntityKind::Author, EntityKind::Series, EntityKind::Book] {
            assert_eq!(decide(kind, Remote::Deleted, true, None), Action::Unlink);
            assert_eq!(decide(kind, Remote::Deleted, false, None), Action::Delete);
        }
    }

    #[test]
    fn a_merged_book_never_merges_two_books_holding_user_data() {
        let t = Uuid::now_v7();
        let merged = Remote::MergedInto(9);
        let book = EntityKind::Book;
        assert_eq!(decide(book, merged, false, Some((t, true))), Action::Delete);
        assert_eq!(
            decide(book, merged, true, Some((t, false))),
            Action::ReplaceTarget {
                target: t,
                canonical: 9
            }
        );
        assert_eq!(
            decide(book, merged, true, Some((t, true))),
            Action::FlagConflict {
                target: t,
                canonical: 9
            }
        );
        assert_eq!(decide(book, merged, false, None), Action::Delete);
        assert_eq!(decide(book, merged, true, None), Action::Repoint(9));
    }

    #[test]
    fn a_merged_series_or_author_folds_into_its_survivor() {
        let t = Uuid::now_v7();
        for kind in [EntityKind::Author, EntityKind::Series] {
            let merged = Remote::MergedInto(9);
            assert_eq!(
                decide(kind, merged, true, Some((t, true))),
                Action::MergeInto(t)
            );
            assert_eq!(decide(kind, merged, false, None), Action::Repoint(9));
        }
    }
}
