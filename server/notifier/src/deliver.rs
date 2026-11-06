//! Sending webhooks: sign, POST, record, and schedule the next try.

use std::time::Duration;

use alloy::hex;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use sqlx::PgPool;

/// A shop's server gets this long to answer.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// Wait before try 2, 3, 4 and 5; after the fifth try fails the webhook is marked failed.
const BACKOFF_SECONDS: [i64; 4] = [10, 60, 300, 1800];
/// Rows sent per pass.
const BATCH: i64 = 20;
/// How much of a failed reply to keep in the log.
const ERROR_TEXT: usize = 300;

/// `hex(HMAC_SHA256(body, secret))`, the `X-Uniform-Signature` header (SPEC).
pub fn signature(secret: &str, body: &str) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(body.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// When to try again after `attempts` failed tries, or `None` to give up.
pub fn next_try(attempts: i32, now: i64) -> Option<i64> {
    let index = usize::try_from(attempts).ok()?.checked_sub(1)?;
    BACKOFF_SECONDS.get(index).map(|wait| now + wait)
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    url: String,
    body: String,
    attempts: i32,
    event: String,
    order_id: String,
    secret: String,
}

/// Sends every due webhook once. Returns a log line per webhook.
pub async fn deliver_due(pool: &PgPool, http: &reqwest::Client, now: i64) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT w.id, w.url, w.body, w.attempts, w.event, w.order_id, m.webhook_secret AS secret
         FROM webhook_log w JOIN merchants m ON m.id = w.merchant_id
         WHERE w.status = 'pending' AND w.next_attempt_at <= $1
         ORDER BY w.next_attempt_at, w.id
         LIMIT $2",
    )
    .bind(now)
    .bind(BATCH)
    .fetch_all(pool)
    .await?;

    let mut lines = Vec::with_capacity(rows.len());
    for row in rows {
        let sent = http
            .post(&row.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("X-Uniform-Signature", signature(&row.secret, &row.body))
            .header("X-Uniform-Event", &row.event)
            .body(row.body.clone())
            .send()
            .await;
        let attempts = row.attempts + 1;
        let (code, problem) = match sent {
            Ok(response) if response.status().is_success() => (Some(response.status().as_u16()), None),
            Ok(response) => {
                let code = response.status().as_u16();
                let text: String = response.text().await.unwrap_or_default().chars().take(ERROR_TEXT).collect();
                (Some(code), Some(format!("HTTP {code} {text}").trim().to_owned()))
            }
            // Without the URL: it is the shop's, and may carry a token.
            Err(error) => (None, Some(error.without_url().to_string())),
        };
        let code = code.map(i32::from);
        match problem {
            None => {
                sqlx::query(
                    "UPDATE webhook_log SET status = 'delivered', attempts = $2, last_status = $3, last_error = NULL,
                                            delivered_at = $4
                     WHERE id = $1",
                )
                .bind(row.id)
                .bind(attempts)
                .bind(code)
                .bind(now)
                .execute(pool)
                .await?;
                lines.push(format!("{} for order {} delivered", row.event, row.order_id));
            }
            Some(problem) => {
                let next = next_try(attempts, now);
                sqlx::query(
                    "UPDATE webhook_log SET status = $2, attempts = $3, last_status = $4, last_error = $5,
                                            next_attempt_at = COALESCE($6, next_attempt_at)
                     WHERE id = $1",
                )
                .bind(row.id)
                .bind(if next.is_some() { "pending" } else { "failed" })
                .bind(attempts)
                .bind(code)
                .bind(&problem)
                .bind(next)
                .execute(pool)
                .await?;
                let when = next.map_or_else(|| "giving up".to_owned(), |at| format!("next try in {} s", at - now));
                lines.push(format!("{} for order {} failed ({problem}); {when}", row.event, row.order_id));
            }
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same value as `printf '%s' '{"a":1}' | openssl dgst -sha256 -hmac whsec_test`.
    #[test]
    fn the_signature_is_hex_hmac_sha256_of_the_body() {
        assert_eq!(
            signature("whsec_test", r#"{"a":1}"#),
            "51426af50a41dd7ff2cd3f116594734766d4018d15d6fb07169aee5d2959adf5"
        );
    }

    /// A one-request HTTP server: answers `status` and hands back the request it read.
    async fn shop_server(status: u16) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            // Headers, then the body (`{"a":1}` in these tests).
            while !String::from_utf8_lossy(&request).ends_with("}") {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let reply = format!("HTTP/1.1 {status} X\r\ncontent-length: 2\r\nconnection: close\r\n\r\nno");
            socket.write_all(reply.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (url, task)
    }

    async fn one_pending_webhook(pool: &PgPool, url: &str) {
        sqlx::query(
            "INSERT INTO merchants (name, payout_address, webhook_secret, created_at)
             VALUES ('Kettle & Co', '0x5a30000000000000000000000000000000000001', 'whsec_test', 1)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO orders (id, merchant_id, amount, reference, status, created_at, expires_at)
             SELECT $1, id, 1, 'r', 'paid', 1, 2 FROM merchants",
        )
        .bind(format!("0x{}", "ab".repeat(32)))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO webhook_log (merchant_id, order_id, event, url, body, next_attempt_at, created_at)
             SELECT m.id, o.id, 'order.paid', $1, '{\"a\":1}', 0, 0 FROM merchants m, orders o",
        )
        .bind(url)
        .execute(pool)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "../order-desk/migrations")]
    async fn a_2xx_is_delivered_with_the_signature(pool: PgPool) {
        let (url, server) = shop_server(200).await;
        one_pending_webhook(&pool, &url).await;
        deliver_due(&pool, &reqwest::Client::new(), 100).await.unwrap();

        let request = server.await.unwrap().to_lowercase();
        assert!(request.contains("x-uniform-signature: 51426af50a41dd7ff2cd3f116594734766d4018d15d6fb07169aee5d2959adf5"));
        let row: (String, i32, Option<i32>) =
            sqlx::query_as("SELECT status, attempts, last_status FROM webhook_log").fetch_one(&pool).await.unwrap();
        assert_eq!(row, ("delivered".to_owned(), 1, Some(200)));
    }

    #[sqlx::test(migrations = "../order-desk/migrations")]
    async fn an_error_is_tried_again_later(pool: PgPool) {
        let (url, server) = shop_server(500).await;
        one_pending_webhook(&pool, &url).await;
        deliver_due(&pool, &reqwest::Client::new(), 100).await.unwrap();
        server.await.unwrap();

        let row: (String, i32, i64, Option<String>) =
            sqlx::query_as("SELECT status, attempts, next_attempt_at, last_error FROM webhook_log")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!((row.0.as_str(), row.1, row.2), ("pending", 1, 110));
        assert_eq!(row.3.as_deref(), Some("HTTP 500 no"));
        assert!(deliver_due(&pool, &reqwest::Client::new(), 105).await.unwrap().is_empty(), "not due yet");
    }

    #[test]
    fn retries_back_off_then_stop() {
        assert_eq!(next_try(1, 100), Some(110));
        assert_eq!(next_try(2, 100), Some(160));
        assert_eq!(next_try(4, 100), Some(1900));
        assert_eq!(next_try(5, 100), None);
        assert_eq!(next_try(0, 100), None);
    }
}
