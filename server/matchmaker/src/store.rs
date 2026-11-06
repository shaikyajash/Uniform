//! The Matchmaker's own SQLite file: where it stopped reading each chain, Bob's fills, logs whose
//! handling failed and will be retried, and chain events not yet accepted by the Order Desk.
//! Separate from the Order Desk's database; the only link between them is HTTP.

use alloy::primitives::{B256, U256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS cursors (
    chain_id   INTEGER PRIMARY KEY,
    next_block INTEGER NOT NULL
);
-- One row per destination lock Bob opened. status: filled -> settled -> claimed, or
-- filled -> refunded; missed when the shop was paid but the source lock could not be redeemed.
-- source_done: the customer's source lock no longer needs a refund from us.
CREATE TABLE IF NOT EXISTS fills (
    destination_lock_id TEXT PRIMARY KEY,
    order_id            TEXT NOT NULL,
    src_chain_id        INTEGER NOT NULL,
    src_lock_id         TEXT NOT NULL,
    src_expiry          INTEGER NOT NULL,
    destination_expiry  INTEGER NOT NULL,
    amount              TEXT NOT NULL,
    status              TEXT NOT NULL,
    secret              TEXT,
    source_done         INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS fills_by_order ON fills (order_id);
-- Logs whose handling failed. Retried every pass without holding up the chain's cursor.
CREATE TABLE IF NOT EXISTS pending_logs (
    chain_id  INTEGER NOT NULL,
    tx_hash   TEXT NOT NULL,
    log_index INTEGER NOT NULL,
    source    TEXT NOT NULL,
    log_json  TEXT NOT NULL,
    failures  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (chain_id, tx_hash, log_index)
);
-- Events waiting to be accepted by the Order Desk.
CREATE TABLE IF NOT EXISTS outbox (
    chain_id  INTEGER NOT NULL,
    tx_hash   TEXT NOT NULL,
    log_index INTEGER NOT NULL,
    body      TEXT NOT NULL,
    PRIMARY KEY (chain_id, tx_hash, log_index)
);
-- Events the Order Desk refused for good, kept for a person to look at.
CREATE TABLE IF NOT EXISTS dead_letters (
    chain_id  INTEGER NOT NULL,
    tx_hash   TEXT NOT NULL,
    log_index INTEGER NOT NULL,
    body      TEXT NOT NULL,
    reason    TEXT NOT NULL,
    PRIMARY KEY (chain_id, tx_hash, log_index)
);
";

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

#[derive(Debug, Clone)]
pub struct Fill {
    pub order_id: B256,
    pub src_chain_id: u64,
    pub src_lock_id: B256,
    pub src_expiry: u64,
    pub destination_lock_id: B256,
    pub destination_expiry: u64,
    pub amount: U256,
    pub status: String,
    pub secret: Option<B256>,
}

#[derive(Debug, Clone)]
pub struct PendingLog {
    pub chain_id: u64,
    pub tx_hash: String,
    pub log_index: u64,
    pub source: String,
    pub log_json: String,
}

impl Store {
    pub async fn open(path: &str) -> Result<Store, sqlx::Error> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::raw_sql(SCHEMA).execute(&pool).await?;
        Ok(Store { pool })
    }

    pub async fn cursor(&self, chain_id: u64) -> Result<Option<u64>, sqlx::Error> {
        let row = sqlx::query("SELECT next_block FROM cursors WHERE chain_id = ?")
            .bind(chain_id as i64)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get::<i64, _>(0) as u64))
    }

    pub async fn set_cursor(&self, chain_id: u64, next_block: u64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO cursors (chain_id, next_block) VALUES (?, ?)
             ON CONFLICT (chain_id) DO UPDATE SET next_block = excluded.next_block",
        )
        .bind(chain_id as i64)
        .bind(next_block as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Whether Bob has filled this order before (the Matchmaker fills each order at most once).
    pub async fn has_fill(&self, order_id: B256) -> Result<bool, sqlx::Error> {
        let row = sqlx::query("SELECT 1 FROM fills WHERE order_id = ?")
            .bind(order_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    /// Records a fill unless this destination lock is already recorded. Returns whether it was new.
    pub async fn record_fill_if_missing(&self, f: &Fill) -> Result<bool, sqlx::Error> {
        let done = sqlx::query(
            "INSERT INTO fills (destination_lock_id, order_id, src_chain_id, src_lock_id, src_expiry,
                                destination_expiry, amount, status, secret)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(f.destination_lock_id.to_string())
        .bind(f.order_id.to_string())
        .bind(f.src_chain_id as i64)
        .bind(f.src_lock_id.to_string())
        .bind(f.src_expiry as i64)
        .bind(f.destination_expiry as i64)
        .bind(f.amount.to_string())
        .bind(&f.status)
        .bind(f.secret.map(|s| s.to_string()))
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(done == 1)
    }

    pub async fn fill_by_destination(
        &self,
        destination_lock_id: B256,
    ) -> Result<Option<Fill>, sqlx::Error> {
        let row = sqlx::query("SELECT * FROM fills WHERE destination_lock_id = ?")
            .bind(destination_lock_id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| to_fill(&r)).transpose()
    }

    pub async fn fills_with_status(&self, status: &str) -> Result<Vec<Fill>, sqlx::Error> {
        let rows = sqlx::query("SELECT * FROM fills WHERE status = ?")
            .bind(status)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(to_fill).collect()
    }

    /// Fills whose source lock may still need refunding to the customer: past expiry, not done,
    /// and not redeemed by Bob.
    pub async fn source_refunds_due(
        &self,
        chain_id: u64,
        now: u64,
        limit: u32,
    ) -> Result<Vec<Fill>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT * FROM fills WHERE src_chain_id = ? AND source_done = 0 AND src_expiry <= ?
             AND status IN ('refunded', 'missed', 'filled') LIMIT ?",
        )
        .bind(chain_id as i64)
        .bind(now as i64)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(to_fill).collect()
    }

    pub async fn source_done(&self, destination_lock_id: B256) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE fills SET source_done = 1 WHERE destination_lock_id = ?")
            .bind(destination_lock_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Records the secret the first time a fill is settled. A second report changes nothing.
    pub async fn mark_settled(
        &self,
        destination_lock_id: B256,
        secret: B256,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE fills SET status = 'settled', secret = ? WHERE destination_lock_id = ? AND status = 'filled'")
            .bind(secret.to_string())
            .bind(destination_lock_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_fill_status(
        &self,
        destination_lock_id: B256,
        status: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE fills SET status = ? WHERE destination_lock_id = ?")
            .bind(status)
            .bind(destination_lock_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn park_log(&self, p: &PendingLog) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO pending_logs (chain_id, tx_hash, log_index, source, log_json) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(p.chain_id as i64)
        .bind(&p.tx_hash)
        .bind(p.log_index as i64)
        .bind(&p.source)
        .bind(&p.log_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn pending_logs(&self) -> Result<Vec<PendingLog>, sqlx::Error> {
        let rows = sqlx::query("SELECT chain_id, tx_hash, log_index, source, log_json FROM pending_logs ORDER BY rowid")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .map(|r| PendingLog {
                chain_id: r.get::<i64, _>(0) as u64,
                tx_hash: r.get(1),
                log_index: r.get::<i64, _>(2) as u64,
                source: r.get(3),
                log_json: r.get(4),
            })
            .collect())
    }

    /// Counts a permanent failure for a parked log and returns the total.
    pub async fn bump_pending(
        &self,
        chain_id: u64,
        tx_hash: &str,
        log_index: u64,
    ) -> Result<u64, sqlx::Error> {
        let row = sqlx::query(
            "UPDATE pending_logs SET failures = failures + 1
             WHERE chain_id = ? AND tx_hash = ? AND log_index = ? RETURNING failures",
        )
        .bind(chain_id as i64)
        .bind(tx_hash)
        .bind(log_index as i64)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.get::<i64, _>(0) as u64).unwrap_or(0))
    }

    pub async fn drop_pending(
        &self,
        chain_id: u64,
        tx_hash: &str,
        log_index: u64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "DELETE FROM pending_logs WHERE chain_id = ? AND tx_hash = ? AND log_index = ?",
        )
        .bind(chain_id as i64)
        .bind(tx_hash)
        .bind(log_index as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn enqueue(
        &self,
        chain_id: u64,
        tx_hash: B256,
        log_index: u64,
        body: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO outbox (chain_id, tx_hash, log_index, body) VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING")
            .bind(chain_id as i64)
            .bind(tx_hash.to_string())
            .bind(log_index as i64)
            .bind(body)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn outbox(&self) -> Result<Vec<(u64, String, u64, String)>, sqlx::Error> {
        let rows =
            sqlx::query("SELECT chain_id, tx_hash, log_index, body FROM outbox ORDER BY rowid")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get::<i64, _>(0) as u64,
                    r.get(1),
                    r.get::<i64, _>(2) as u64,
                    r.get(3),
                )
            })
            .collect())
    }

    pub async fn dequeue(
        &self,
        chain_id: u64,
        tx_hash: &str,
        log_index: u64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM outbox WHERE chain_id = ? AND tx_hash = ? AND log_index = ?")
            .bind(chain_id as i64)
            .bind(tx_hash)
            .bind(log_index as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Moves a refused event from the outbox to `dead_letters`.
    pub async fn dead_letter(
        &self,
        chain_id: u64,
        tx_hash: &str,
        log_index: u64,
        body: &str,
        reason: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO dead_letters (chain_id, tx_hash, log_index, body, reason) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(chain_id as i64)
        .bind(tx_hash)
        .bind(log_index as i64)
        .bind(body)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        self.dequeue(chain_id, tx_hash, log_index).await
    }
}

fn parse_b256(text: &str) -> Result<B256, sqlx::Error> {
    text.parse().map_err(|e| sqlx::Error::Decode(Box::new(e)))
}

fn to_fill(r: &sqlx::sqlite::SqliteRow) -> Result<Fill, sqlx::Error> {
    Ok(Fill {
        order_id: parse_b256(&r.get::<String, _>("order_id"))?,
        src_chain_id: r.get::<i64, _>("src_chain_id") as u64,
        src_lock_id: parse_b256(&r.get::<String, _>("src_lock_id"))?,
        src_expiry: r.get::<i64, _>("src_expiry") as u64,
        destination_lock_id: parse_b256(&r.get::<String, _>("destination_lock_id"))?,
        destination_expiry: r.get::<i64, _>("destination_expiry") as u64,
        amount: r
            .get::<String, _>("amount")
            .parse()
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        status: r.get("status"),
        secret: r
            .get::<Option<String>, _>("secret")
            .map(|s| parse_b256(&s))
            .transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> Store {
        Store::open(":memory:").await.unwrap()
    }

    fn fill() -> Fill {
        Fill {
            order_id: B256::repeat_byte(1),
            src_chain_id: 84532,
            src_lock_id: B256::repeat_byte(2),
            src_expiry: 2000,
            destination_lock_id: B256::repeat_byte(3),
            destination_expiry: 1500,
            amount: U256::from(12_000_000u64),
            status: "filled".into(),
            secret: None,
        }
    }

    #[tokio::test]
    async fn cursors_start_empty_and_update() {
        let s = store().await;
        assert_eq!(s.cursor(84532).await.unwrap(), None);
        s.set_cursor(84532, 10).await.unwrap();
        s.set_cursor(84532, 20).await.unwrap();
        assert_eq!(s.cursor(84532).await.unwrap(), Some(20));
    }

    #[tokio::test]
    async fn a_fill_is_recorded_once_and_settles_once() {
        let s = store().await;
        assert!(s.record_fill_if_missing(&fill()).await.unwrap());
        assert!(s.has_fill(B256::repeat_byte(1)).await.unwrap());
        assert!(!s.record_fill_if_missing(&fill()).await.unwrap());

        s.mark_settled(B256::repeat_byte(3), B256::repeat_byte(9))
            .await
            .unwrap();
        s.mark_settled(B256::repeat_byte(3), B256::repeat_byte(8))
            .await
            .unwrap();
        let f = s
            .fill_by_destination(B256::repeat_byte(3))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(f.status, "settled");
        assert_eq!(f.secret, Some(B256::repeat_byte(9)));
        assert_eq!(s.fills_with_status("settled").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn two_destination_locks_for_one_order_are_both_kept() {
        let s = store().await;
        s.record_fill_if_missing(&fill()).await.unwrap();
        let mut second = fill();
        second.destination_lock_id = B256::repeat_byte(4);
        assert!(s.record_fill_if_missing(&second).await.unwrap());
        assert!(s
            .fill_by_destination(B256::repeat_byte(4))
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn source_refunds_are_due_only_for_our_expired_unredeemed_fills() {
        let s = store().await;
        s.record_fill_if_missing(&fill()).await.unwrap();
        assert!(s
            .source_refunds_due(84532, 1999, 10)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            s.source_refunds_due(84532, 2000, 10).await.unwrap().len(),
            1
        );
        s.set_fill_status(B256::repeat_byte(3), "claimed")
            .await
            .unwrap();
        assert!(s
            .source_refunds_due(84532, 2000, 10)
            .await
            .unwrap()
            .is_empty());
        s.set_fill_status(B256::repeat_byte(3), "refunded")
            .await
            .unwrap();
        s.source_done(B256::repeat_byte(3)).await.unwrap();
        assert!(s
            .source_refunds_due(84532, 2000, 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn parked_logs_count_failures_and_drop() {
        let s = store().await;
        let p = PendingLog {
            chain_id: 1,
            tx_hash: "0xab".into(),
            log_index: 2,
            source: "SourceLock".into(),
            log_json: "{}".into(),
        };
        s.park_log(&p).await.unwrap();
        s.park_log(&p).await.unwrap();
        assert_eq!(s.pending_logs().await.unwrap().len(), 1);
        assert_eq!(s.bump_pending(1, "0xab", 2).await.unwrap(), 1);
        assert_eq!(s.bump_pending(1, "0xab", 2).await.unwrap(), 2);
        s.drop_pending(1, "0xab", 2).await.unwrap();
        assert!(s.pending_logs().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn outbox_keeps_one_copy_and_refused_events_are_kept_aside() {
        let s = store().await;
        let tx = B256::repeat_byte(5);
        s.enqueue(1874, tx, 0, "{}").await.unwrap();
        s.enqueue(1874, tx, 0, "{}").await.unwrap();
        let items = s.outbox().await.unwrap();
        assert_eq!(items.len(), 1);
        s.dead_letter(1874, &items[0].1, 0, "{}", "400")
            .await
            .unwrap();
        assert!(s.outbox().await.unwrap().is_empty());
    }
}
