//! Data operations for following a metadata provider's merges and deletions.
//!
//! Every delete re-checks `book_has_user_data` in the same statement, so a book that
//! picked up a file or a want between the decision and the write survives.

use sqlx::{Executor, PgPool, Postgres};
use uuid::Uuid;

use super::Result;

/// The entity tables that carry a provider id in `identifiers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityKind {
    Author,
    Series,
    Book,
}

impl EntityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntityKind::Author => "author",
            EntityKind::Series => "series",
            EntityKind::Book => "book",
        }
    }

    fn table(self) -> &'static str {
        match self {
            EntityKind::Author => "authors",
            EntityKind::Series => "series",
            EntityKind::Book => "books",
        }
    }
}

/// Why a link needs a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    /// The provider deleted it, and we hold user data for it, so it was unlinked.
    Deleted,
    /// The provider merged it into an entity we also hold user data for.
    MergeConflict,
}

impl IssueKind {
    fn as_str(self) -> &'static str {
        match self {
            IssueKind::Deleted => "deleted",
            IssueKind::MergeConflict => "merge_conflict",
        }
    }
}

/// Every row of `kind` linked to the provider, with the provider id parsed. Ids that
/// are not plain integers are skipped: the provider could never have issued them.
pub async fn list_linked(
    pool: &PgPool,
    kind: EntityKind,
    provider: &str,
) -> Result<Vec<(Uuid, i64)>> {
    let sql = format!(
        "SELECT id, (identifiers->>$1)::bigint FROM {} WHERE identifiers->>$1 ~ '^[0-9]{{1,18}}$'",
        kind.table()
    );
    Ok(
        sqlx::query_as::<_, (Uuid, i64)>(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(provider)
            .fetch_all(pool)
            .await?,
    )
}

/// The provider id of one row, if it has a usable one.
pub async fn list_linked_one(
    pool: &PgPool,
    kind: EntityKind,
    id: Uuid,
    provider: &str,
) -> Result<Option<i64>> {
    let sql = format!(
        "SELECT (identifiers->>$1)::bigint FROM {} \
         WHERE id = $2 AND identifiers->>$1 ~ '^[0-9]{{1,18}}$'",
        kind.table()
    );
    Ok(
        sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(provider)
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn exists<'e, E>(exec: E, kind: EntityKind, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let sql = format!(
        "SELECT EXISTS (SELECT 1 FROM {} WHERE id = $1)",
        kind.table()
    );
    Ok(
        sqlx::query_scalar::<_, bool>(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(id)
            .fetch_one(exec)
            .await?,
    )
}

/// The oldest other row of `kind` carrying `external_id`.
pub async fn find_linked<'e, E>(
    exec: E,
    kind: EntityKind,
    provider: &str,
    external_id: i64,
    exclude: Uuid,
) -> Result<Option<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    let sql = format!(
        "SELECT id FROM {} WHERE identifiers->>$1 = $2 AND id <> $3 ORDER BY created_at LIMIT 1",
        kind.table()
    );
    Ok(
        sqlx::query_scalar::<_, Uuid>(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(provider)
            .bind(external_id.to_string())
            .bind(exclude)
            .fetch_optional(exec)
            .await?,
    )
}

/// Whether deleting this entity would lose something a user or the library put there.
/// For a series or author that means any of its books has user data.
pub async fn has_user_data<'e, E>(exec: E, kind: EntityKind, id: Uuid) -> Result<bool>
where
    E: Executor<'e, Database = Postgres>,
{
    let attached = match kind {
        EntityKind::Book => {
            sqlx::query_scalar!(r#"SELECT book_has_user_data($1) AS "attached!""#, id)
                .fetch_one(exec)
                .await?
        }
        EntityKind::Series => {
            sqlx::query_scalar!(
                r#"SELECT EXISTS (
                       SELECT 1 FROM book_series
                       WHERE series_id = $1 AND book_has_user_data(book_id)
                   ) AS "attached!""#,
                id
            )
            .fetch_one(exec)
            .await?
        }
        EntityKind::Author => {
            sqlx::query_scalar!(
                r#"SELECT (
                       EXISTS (SELECT 1 FROM book_authors
                               WHERE author_id = $1 AND book_has_user_data(book_id))
                       OR EXISTS (SELECT 1 FROM books
                                  WHERE discovered_from_author_id = $1
                                    AND book_has_user_data(id))
                   ) AS "attached!""#,
                id
            )
            .fetch_one(exec)
            .await?
        }
    };
    Ok(attached)
}

/// Point the row at the provider's canonical id. The slug belonged to the old id, so
/// it goes; series and authors are made due so their next sync fills in the rest.
pub async fn repoint<'e, E>(
    exec: E,
    kind: EntityKind,
    id: Uuid,
    provider: &str,
    new_external_id: i64,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let due = match kind {
        EntityKind::Book => "",
        EntityKind::Series | EntityKind::Author => ", next_metadata_refresh_at = NOW()",
    };
    let sql = format!(
        "UPDATE {} SET identifiers = (COALESCE(identifiers, '{{}}'::jsonb) - ($2 || '_slug')) \
         || jsonb_build_object($2, $3::text), updated_at = NOW(){due} WHERE id = $1",
        kind.table()
    );
    sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .bind(provider)
        .bind(new_external_id.to_string())
        .execute(exec)
        .await?;
    Ok(())
}

/// Drop the provider id (and its slug), leaving the row and everything attached to it.
/// A series or author without a provider id is no longer synced from it.
pub async fn unlink<'e, E>(exec: E, kind: EntityKind, id: Uuid, provider: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let sql = format!(
        "UPDATE {} SET identifiers = COALESCE(identifiers, '{{}}'::jsonb) - $2 - ($2 || '_slug'), \
         updated_at = NOW() WHERE id = $1",
        kind.table()
    );
    sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .bind(provider)
        .execute(exec)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn flag<'e, E>(
    exec: E,
    kind: EntityKind,
    id: Uuid,
    provider: &str,
    external_id: i64,
    issue: IssueKind,
    canonical_external_id: Option<i64>,
    conflicting_entity_id: Option<Uuid>,
) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO provider_link_issues
            (entity_type, entity_id, provider, external_id, kind,
             canonical_external_id, conflicting_entity_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (entity_type, entity_id, provider) DO UPDATE SET
            external_id = EXCLUDED.external_id,
            kind = EXCLUDED.kind,
            canonical_external_id = EXCLUDED.canonical_external_id,
            conflicting_entity_id = EXCLUDED.conflicting_entity_id,
            detected_at = NOW()
        "#,
        kind.as_str(),
        id,
        provider,
        external_id.to_string(),
        issue.as_str(),
        canonical_external_id.map(|c| c.to_string()),
        conflicting_entity_id,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Clear a flag once the link is healthy again.
pub async fn clear_flag<'e, E>(exec: E, kind: EntityKind, id: Uuid, provider: &str) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        "DELETE FROM provider_link_issues WHERE entity_type = $1 AND entity_id = $2 AND provider = $3",
        kind.as_str(),
        id,
        provider,
    )
    .execute(exec)
    .await?;
    Ok(())
}

/// Drop flags whose entity no longer exists.
pub async fn prune_flags(pool: &PgPool) -> Result<u64> {
    let result = sqlx::query!(
        r#"
        DELETE FROM provider_link_issues i
        WHERE (i.entity_type = 'book' AND NOT EXISTS (SELECT 1 FROM books WHERE id = i.entity_id))
           OR (i.entity_type = 'series' AND NOT EXISTS (SELECT 1 FROM series WHERE id = i.entity_id))
           OR (i.entity_type = 'author' AND NOT EXISTS (SELECT 1 FROM authors WHERE id = i.entity_id))
        "#
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Delete those of `ids` that carry no user data, returning what was deleted.
pub async fn delete_books_without_user_data<'e, E>(exec: E, ids: &[Uuid]) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!(
        "DELETE FROM books WHERE id = ANY($1) AND NOT book_has_user_data(id) RETURNING id",
        ids,
    )
    .fetch_all(exec)
    .await?)
}

/// Delete those of `ids` that nothing holds any more: no user data, not discovered
/// from an author (author sync owns those), and in no series. Run after a series is
/// removed, for the books only that series had brought in.
pub async fn delete_unanchored_books<'e, E>(exec: E, ids: &[Uuid]) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!(
        r#"
        DELETE FROM books
        WHERE id = ANY($1)
          AND discovered_from_author_id IS NULL
          AND NOT EXISTS (SELECT 1 FROM book_series WHERE book_id = books.id)
          AND NOT book_has_user_data(id)
        RETURNING id
        "#,
        ids,
    )
    .fetch_all(exec)
    .await?)
}

/// Take books out of a series when Hardcover no longer lists them in it, unless they
/// hold user data. A book with no Hardcover id compares NULL and stays: it was put
/// there by the library or a person, not by Hardcover. Returns the books taken out.
pub async fn remove_stale_series_members<'e, E>(
    exec: E,
    series_id: Uuid,
    current_hardcover_ids: &[String],
) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!(
        r#"
        DELETE FROM book_series bs
        USING books b
        WHERE bs.series_id = $1
          AND b.id = bs.book_id
          AND NOT (b.identifiers->>'hardcover' = ANY($2))
          AND NOT book_has_user_data(b.id)
        RETURNING bs.book_id
        "#,
        series_id,
        current_hardcover_ids,
    )
    .fetch_all(exec)
    .await?)
}

/// Delete the books discovered from an author that carry no user data.
pub async fn delete_discovered_books_without_user_data<'e, E>(
    exec: E,
    author_id: Uuid,
) -> Result<Vec<Uuid>>
where
    E: Executor<'e, Database = Postgres>,
{
    Ok(sqlx::query_scalar!(
        "DELETE FROM books WHERE discovered_from_author_id = $1 AND NOT book_has_user_data(id) RETURNING id",
        author_id,
    )
    .fetch_all(exec)
    .await?)
}

/// Carry monitoring from a series or author being merged away onto its survivor,
/// without overriding a choice already made on the survivor.
pub async fn carry_monitoring<'e, E>(exec: E, kind: EntityKind, from: Uuid, to: Uuid) -> Result<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let sql = match kind {
        EntityKind::Book => return Ok(()),
        EntityKind::Series => {
            r#"
            UPDATE series t SET
                is_monitored = t.is_monitored OR s.is_monitored,
                auto_want_mode = CASE
                    WHEN t.auto_want_mode IN ('none', 'inherit')
                     AND s.auto_want_mode NOT IN ('none', 'inherit')
                    THEN s.auto_want_mode ELSE t.auto_want_mode END,
                monitoring_started_at = COALESCE(t.monitoring_started_at, s.monitoring_started_at),
                next_metadata_refresh_at = NOW(),
                updated_at = NOW()
            FROM series s WHERE t.id = $2 AND s.id = $1
            "#
        }
        EntityKind::Author => {
            r#"
            UPDATE authors t SET
                is_monitored = t.is_monitored OR s.is_monitored,
                auto_want_mode = CASE
                    WHEN t.auto_want_mode = 'none' AND s.auto_want_mode <> 'none'
                    THEN s.auto_want_mode ELSE t.auto_want_mode END,
                monitoring_started_at = COALESCE(t.monitoring_started_at, s.monitoring_started_at),
                next_metadata_refresh_at = NOW(),
                updated_at = NOW()
            FROM authors s WHERE t.id = $2 AND s.id = $1
            "#
        }
    };
    sqlx::query(sql).bind(from).bind(to).execute(exec).await?;
    Ok(())
}

/// Hand an author's discovered books to the author it was merged into.
pub async fn move_discovered_books<'e, E>(exec: E, from: Uuid, to: Uuid) -> Result<u64>
where
    E: Executor<'e, Database = Postgres>,
{
    let result = sqlx::query!(
        "UPDATE books SET discovered_from_author_id = $2 WHERE discovered_from_author_id = $1",
        from,
        to,
    )
    .execute(exec)
    .await?;
    Ok(result.rows_affected())
}

/// A flagged link, with the entity's display name.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LinkIssueRow {
    pub entity_type: String,
    pub entity_id: Uuid,
    pub name: Option<String>,
    pub provider: String,
    pub external_id: String,
    pub kind: String,
    pub canonical_external_id: Option<String>,
    pub conflicting_entity_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub detected_at: time::OffsetDateTime,
}

pub async fn list_issues(pool: &PgPool) -> Result<Vec<LinkIssueRow>> {
    Ok(sqlx::query_as!(
        LinkIssueRow,
        r#"
        SELECT i.entity_type, i.entity_id, COALESCE(b.title, s.name, a.name) AS name,
               i.provider, i.external_id, i.kind, i.canonical_external_id,
               i.conflicting_entity_id, i.detected_at
        FROM provider_link_issues i
        LEFT JOIN books b ON i.entity_type = 'book' AND b.id = i.entity_id
        LEFT JOIN series s ON i.entity_type = 'series' AND s.id = i.entity_id
        LEFT JOIN authors a ON i.entity_type = 'author' AND a.id = i.entity_id
        ORDER BY i.detected_at DESC
        "#
    )
    .fetch_all(pool)
    .await?)
}
