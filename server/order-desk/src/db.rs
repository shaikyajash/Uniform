//! The Order Desk's Postgres: one connection pool, migrated at startup.

use std::fmt;

use sqlx::{
    PgPool,
    migrate::{MigrateError, Migrator},
    postgres::PgPoolOptions,
};

/// The files in `order-desk/migrations/`, built into the binary so a deploy cannot miss one.
static MIGRATOR: Migrator = sqlx::migrate!();

/// Open connections at once. The API is small; Postgres allows 100 by default.
const MAX_CONNECTIONS: u32 = 10;

#[derive(Debug)]
pub enum DbError {
    Connect(sqlx::Error),
    Migrate(MigrateError),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::Connect(error) => write!(f, "cannot connect to Postgres: {error}"),
            DbError::Migrate(error) => write!(f, "cannot run the migrations: {error}"),
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DbError::Connect(error) => Some(error),
            DbError::Migrate(error) => Some(error),
        }
    }
}

/// Connects to `database_url` and applies any migration not yet applied.
///
/// Safe to run on every start: applied migrations are recorded in `_sqlx_migrations` and skipped,
/// and a migration file that changed after it was applied stops the start with an error.
pub async fn connect(database_url: &str) -> Result<PgPool, DbError> {
    let pool = PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .connect(database_url)
        .await
        .map_err(DbError::Connect)?;
    MIGRATOR.run(&pool).await.map_err(DbError::Migrate)?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use common::status::{OrderStatus, PayPath};
    use sqlx::PgPool;

    const ORDER_ID: &str = "0xabababababababababababababababababababababababababababababababab";

    async fn add_order(pool: &PgPool) {
        sqlx::query(
            "INSERT INTO merchants (name, payout_address, webhook_secret, created_at)
             VALUES ('Kettle & Co', '0x5a30000000000000000000000000000000000001', 'whsec_test', 1)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO orders (id, merchant_id, amount, reference, created_at, expires_at)
             VALUES ($1, 1, '12000000'::numeric, 'shop-123', 1, 2)",
        )
        .bind(ORDER_ID)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn set(pool: &PgPool, column: &str, value: &str) -> Result<(), sqlx::Error> {
        let sql = match column {
            "status" => "UPDATE orders SET status = $1 WHERE id = $2",
            "path" => "UPDATE orders SET path = $1 WHERE id = $2",
            other => panic!("no test query for column {other}"),
        };
        sqlx::query(sql)
            .bind(value)
            .bind(ORDER_ID)
            .execute(pool)
            .await
            .map(|_| ())
    }

    /// The status list lives in both `common::status` and the migration; this keeps them equal.
    #[sqlx::test]
    async fn the_database_accepts_every_rust_status_and_path(pool: PgPool) {
        add_order(&pool).await;
        for status in OrderStatus::ALL {
            set(&pool, "status", status.as_str()).await.unwrap_or_else(|error| panic!("{status}: {error}"));
        }
        for path in PayPath::ALL {
            set(&pool, "path", path.as_str()).await.unwrap_or_else(|error| panic!("{path}: {error}"));
        }
    }

    #[sqlx::test]
    async fn the_database_rejects_names_rust_does_not_have(pool: PgPool) {
        add_order(&pool).await;
        assert!(set(&pool, "status", "shop_locked").await.is_err());
        assert!(set(&pool, "path", "bridge").await.is_err());
    }

    #[sqlx::test]
    async fn amounts_read_back_as_exact_text(pool: PgPool) {
        add_order(&pool).await;
        let amount: String = sqlx::query_scalar("SELECT amount::text FROM orders WHERE id = $1")
            .bind(ORDER_ID)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(amount.parse::<common::amount::Amount>().unwrap(), common::amount::Amount::from(12_000_000u64));
    }

    const TX_HASH: &str = "0x7777777777777777777777777777777777777777777777777777777777777777";

    async fn add_job(pool: &PgPool, status: &str, tx_hash: Option<&str>) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO relayer_jobs (order_id, merchant_id, chain_id, kind, args, status, tx_hash, raw_tx, created_at, updated_at)
             VALUES ($1, 1, 1874, 'direct_pay', '{}', $2, $3, CASE WHEN $3 IS NULL THEN NULL ELSE '\\x02'::bytea END, 1, 1)",
        )
        .bind(ORDER_ID)
        .bind(status)
        .bind(tx_hash)
        .execute(pool)
        .await
        .map(|_| ())
    }

    #[sqlx::test]
    async fn the_database_accepts_every_rust_job_status(pool: PgPool) {
        add_order(&pool).await;
        // One at a time: finished jobs do not count against "one live job per order".
        for status in common::jobs::JobStatus::ALL {
            add_job(&pool, status.as_str(), Some(TX_HASH)).await.unwrap_or_else(|error| panic!("{status:?}: {error}"));
            sqlx::query("UPDATE relayer_jobs SET status = 'failed'").execute(&pool).await.unwrap();
        }
        assert!(add_job(&pool, "lost", None).await.is_err());
    }

    #[sqlx::test]
    async fn an_order_has_at_most_one_unfinished_job(pool: PgPool) {
        add_order(&pool).await;
        add_job(&pool, "pending", None).await.unwrap();
        assert!(add_job(&pool, "pending", None).await.is_err(), "a second live job must be refused");

        sqlx::query("UPDATE relayer_jobs SET status = 'failed'").execute(&pool).await.unwrap();
        add_job(&pool, "pending", None).await.expect("after a failure the customer can try again");
    }

    #[sqlx::test]
    async fn a_sent_job_must_have_its_hash(pool: PgPool) {
        add_order(&pool).await;
        assert!(add_job(&pool, "sending", None).await.is_err());
        assert!(add_job(&pool, "sent", None).await.is_err());
        assert!(add_job(&pool, "confirmed", None).await.is_err());
        add_job(&pool, "sent", Some(TX_HASH)).await.unwrap();
    }
}
