//! The alphabet bar's letters, straight from the database.
//!
//! The bars are normally served from the search index, which knows each item's letter as
//! a facet. When that is unavailable the bar used to return 500 and simply vanish from
//! every library page, so this is the same answer computed from the table itself.

use sqlx::{Executor, Postgres, Row};
use uuid::Uuid;

/// The letters that have at least one row in `from` matching `where_clause`, with `#` last
/// when anything files under a non-letter.
///
/// `from` and `where_clause` are SQL the caller writes, never anything a request carries:
/// request values reach the query only through `params`, bound as `$1..$n` in order.
///
/// One pass rather than a letters query plus an EXISTS for `#`: the same CASE that
/// upper-cases a leading letter folds everything else into `#`, and it is the expression
/// the first-letter indexes are built on (migration 0178), so the scan is index-only.
pub async fn available_letters<'e, E>(
    exec: E,
    from: &'static str,
    where_clause: &str,
    params: &[String],
) -> Result<Vec<String>, sqlx::Error>
where
    E: Executor<'e, Database = Postgres>,
{
    // The ordering is a level up because `ORDER BY letter = '#'` is an expression over an
    // output column, which a SELECT DISTINCT will not accept beside the column itself.
    let sql = format!(
        r#"
        SELECT letter FROM (
            SELECT DISTINCT {letter} AS letter
            FROM {from}
            WHERE {where_clause}
        ) present
        ORDER BY letter = '#', letter
        "#,
        letter = LETTER_EXPR,
    );

    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
    for param in params {
        query = query.bind(param);
    }
    let rows = query.fetch_all(exec).await?;
    Ok(rows.iter().map(|r| r.get("letter")).collect())
}

/// The `(id, title)` of the first row in sort order under `letter` (A-Z or `#`), with the
/// same contract for `from`, `where_clause` and `params` as [`available_letters`].
pub async fn first_by_letter<'e, E>(
    exec: E,
    from: &'static str,
    where_clause: &str,
    mut params: Vec<String>,
    letter: &str,
) -> Result<Option<(Uuid, String)>, sqlx::Error>
where
    E: Executor<'e, Database = Postgres>,
{
    params.push(letter.to_uppercase());
    let sql = format!(
        "SELECT id, title FROM {from} WHERE {where_clause} AND {LETTER_EXPR} = ${} \
         ORDER BY COALESCE(sort_title, title) ASC, id ASC LIMIT 1",
        params.len()
    );
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
    for param in &params {
        query = query.bind(param);
    }
    let row = query.fetch_optional(exec).await?;
    Ok(row.map(|r| (r.get("id"), r.get("title"))))
}

/// The first letter of a row's sort key, or `#` when it does not start with one.
///
/// Kept as one string so the indexes in 0178 are built on exactly what the query asks
/// for — an expression index only serves a query that spells it the same way. Unqualified
/// so it reads the same against `books b` and the list queries' unaliased `movies`/`tv_shows`.
pub const LETTER_EXPR: &str = "CASE WHEN COALESCE(sort_title, title) ~ '^[A-Za-z]' \
     THEN UPPER(LEFT(COALESCE(sort_title, title), 1)) ELSE '#' END";
