use sqlx::{Pool, Postgres};

pub type DbPool = Pool<Postgres>;

pub async fn connect(database_url: &str, max_connections: u32) -> Result<DbPool, sqlx::Error> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(max_connections.max(100))
        .min_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .idle_timeout(std::time::Duration::from_secs(30))
        .connect(database_url)
        .await?;
    Ok(pool)
}

pub async fn migrate(pool: &DbPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!().run(pool).await
}
