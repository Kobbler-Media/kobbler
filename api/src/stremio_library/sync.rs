use futures::StreamExt;
use serde::Serialize;
use sqlx::PgPool;
use tracing::warn;

use super::client::{LibraryItem, StremioClient};
use super::progress::classify;
use super::resolve::Resolver;
use super::{history, import};
use crate::repositories::{stremio_accounts, stremio_continue, stremio_history};

const RESOLVE_CONCURRENCY: usize = 4;

#[derive(Debug, Default, Serialize)]
pub struct SyncSummary {
    pub accounts: usize,
    pub items: usize,
    pub errors: Vec<String>,
}

pub async fn sync_all(pool: &PgPool, stremio_api_url: &str) -> SyncSummary {
    let mut summary = SyncSummary::default();
    let targets = match stremio_accounts::list_sync_targets(pool).await {
        Ok(targets) => targets,
        Err(e) => {
            summary
                .errors
                .push(format!("Failed to list Stremio accounts: {e}"));
            return summary;
        }
    };
    let client = StremioClient::new(stremio_api_url);
    let resolver = Resolver::new(pool.clone());

    for target in targets {
        summary.accounts += 1;
        let error = match sync_account(pool, &client, &resolver, &target).await {
            Ok(count) => {
                summary.items += count;
                None
            }
            Err(e) => Some(e),
        };
        if let Some(e) = &error {
            warn!(account_id = %target.id, "Stremio sync: {e}");
            summary.errors.push(e.clone());
        }
        if let Err(e) = stremio_accounts::record_sync(pool, target.id, error.as_deref()).await {
            summary
                .errors
                .push(format!("Failed to record Stremio sync: {e}"));
        }
    }
    summary
}

async fn sync_account(
    pool: &PgPool,
    client: &StremioClient,
    resolver: &Resolver,
    target: &stremio_accounts::SyncTarget,
) -> Result<usize, String> {
    let library = client
        .library(&target.auth_key)
        .await
        .map_err(|e| e.to_string())?;
    let continued = sync_continue(pool, resolver, target.id, &library).await;
    let history_errors = sync_history(pool, resolver, target.id, &library).await;
    match (continued, history_errors.is_empty()) {
        (Ok(count), true) => Ok(count),
        (Ok(_), false) => Err(history_errors.join("; ")),
        (Err(e), true) => Err(e),
        (Err(e), false) => Err(format!("{e}; {}", history_errors.join("; "))),
    }
}

/// A title that fails to resolve is left out and reported, but the rest still land — unless
/// nothing resolved at all, which reads as TMDB being down or unconfigured rather than as an
/// empty Continue Watching, so the previous rows are kept.
async fn sync_continue(
    pool: &PgPool,
    resolver: &Resolver,
    account_id: uuid::Uuid,
    library: &[LibraryItem],
) -> Result<usize, String> {
    let progress: Vec<_> = library.iter().filter_map(classify).collect();
    let attempted = progress.len();

    let results: Vec<_> = futures::stream::iter(progress)
        .map(|p| async move {
            let result = resolver.resolve(&p).await;
            (p, result)
        })
        .buffer_unordered(RESOLVE_CONCURRENCY)
        .collect()
        .await;

    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for (p, result) in results {
        match result {
            Ok(Some(row)) => rows.push(row),
            Ok(None) => {}
            Err(e) => failures.push(format!("{}: {e}", p.imdb_id)),
        }
    }

    if rows.is_empty() && !failures.is_empty() {
        return Err(format!(
            "No titles resolved through TMDB ({} failed, first: {})",
            failures.len(),
            failures[0]
        ));
    }
    stremio_continue::replace_for_account(pool, account_id, &rows)
        .await
        .map_err(|e| e.to_string())?;
    if failures.is_empty() {
        Ok(rows.len())
    } else {
        Err(format!(
            "{} of {attempted} titles failed to resolve (first: {})",
            failures.len(),
            failures[0]
        ))
    }
}

/// Titles that fail to resolve stay unresolved and are retried on the next sync.
async fn sync_history(
    pool: &PgPool,
    resolver: &Resolver,
    account_id: uuid::Uuid,
    library: &[LibraryItem],
) -> Vec<String> {
    let entries: Vec<_> = library.iter().filter_map(history::entry).collect();
    if let Err(e) = stremio_history::store_entries(pool, account_id, &entries).await {
        return vec![format!("Failed to store Stremio history: {e}")];
    }
    let unresolved = match stremio_history::list_unresolved(pool, account_id).await {
        Ok(u) => u,
        Err(e) => return vec![format!("Failed to list Stremio history: {e}")],
    };
    let attempted = unresolved.len();
    let failures: Vec<String> = futures::stream::iter(unresolved)
        .map(|u| async move {
            let identity = resolver
                .identify(&u.stremio_id, u.media_type == "movie")
                .await
                .map_err(|e| format!("{}: {e}", u.stremio_id))?;
            stremio_history::set_identity(pool, account_id, &u.stremio_id, identity.as_ref())
                .await
                .map_err(|e| format!("{}: {e}", u.stremio_id))
        })
        .buffer_unordered(RESOLVE_CONCURRENCY)
        .filter_map(|r| async move { r.err() })
        .collect()
        .await;

    let mut errors = Vec::new();
    if let Some(first) = failures.first() {
        errors.push(format!(
            "{} of {attempted} history titles failed to resolve through TMDB (first: {first})",
            failures.len()
        ));
    }
    errors.extend(import::apply_pending(pool, account_id).await);
    errors
}
