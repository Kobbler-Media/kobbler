//! The alphabet bar's letters, straight from the database.
//!
//! The bars are normally served from the search index, which knows each item's letter as
//! a facet. When that is unavailable the bar used to return 500 and simply vanish from
//! every library page, so this is the same answer computed from the table itself.

use sqlx::{Executor, Postgres, Row};

/// The letters that have at least one row in `from`, with `#` last when anything files
/// under a non-letter.
///
/// `from` and `conditions` are SQL fragments the caller writes, never anything a request
/// carries — the filters a user can set arrive as bound parameters elsewhere or, like the
/// format filter, as one of a fixed set of predicates.
///
/// One pass rather than a letters query plus an EXISTS for `#`: the same CASE that
/// upper-cases a leading letter folds everything else into `#`, and it is the expression
/// the first-letter indexes are built on (migration 0178), so the scan is index-only.
pub async fn available_letters<'e, E>(
    exec: E,
    from: &'static str,
    conditions: &[&'static str],
) -> Result<Vec<String>, sqlx::Error>
where
    E: Executor<'e, Database = Postgres>,
{
    let where_clause = if conditions.is_empty() {
        "TRUE".to_string()
    } else {
        conditions.join(" AND ")
    };

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

    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_all(exec)
        .await?;
    Ok(rows.iter().map(|r| r.get("letter")).collect())
}

/// The first letter of a row's sort key, or `#` when it does not start with one.
///
/// Kept as one string so the indexes in 0178 are built on exactly what the query asks
/// for — an expression index only serves a query that spells it the same way.
pub const LETTER_EXPR: &str = "CASE WHEN COALESCE(b.sort_title, b.title) ~ '^[A-Za-z]' \
     THEN UPPER(LEFT(COALESCE(b.sort_title, b.title), 1)) ELSE '#' END";
