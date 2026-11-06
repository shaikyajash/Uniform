//! What the shop dashboard reads and changes.
//!
//! - `GET /v1/orders`: the shop's orders, newest first, with what was refunded. API key or session.
//! - `GET /v1/payments.csv`: the same as a spreadsheet. API key or session.
//! - `GET /v1/shop`, `PATCH /v1/shop`: the shop's settings. Session only.
//! - `POST /v1/shop/webhook-secret`, `POST /v1/shop/api-key`: new secrets, shown once. Session only.
//! - `GET /v1/webhooks`, `POST /v1/webhooks/{id}/retry`: the webhook log. Session only.

use alloy::hex;
use axum::{
    Json,
    extract::{Path, Query, State, rejection::JsonRejection},
    http::{StatusCode, header},
    response::IntoResponse,
};
use common::amount::Amount;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AppState, now};
use crate::{
    auth::{Owner, Shop, new_api_key},
    error::ApiError,
    random,
};

const USDT_DECIMALS: u8 = 6;
const PAGE: i64 = 50;
const CSV_ROWS: i64 = 5000;
const MAX_URL_LENGTH: usize = 500;

/* ---------- Orders ---------- */

#[derive(Debug, Deserialize)]
pub struct OrdersQuery {
    /// Comma-separated statuses, e.g. `paid,partially_refunded`. All when absent.
    status: Option<String>,
    /// Orders created before this (unix seconds), for the next page.
    before: Option<i64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct OrderRow {
    order_id: String,
    reference: String,
    amount: String,
    status: String,
    path: Option<String>,
    payer: Option<String>,
    created_at: i64,
    paid_at: Option<i64>,
    /// USDT given back so far (confirmed refunds).
    refunded: String,
    /// The latest refund the Relayer is sending or failed to send, if any.
    refund_status: Option<String>,
    refund_error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrdersPage {
    orders: Vec<OrderRow>,
    /// Pass as `before` for older orders; absent on the last page.
    next: Option<i64>,
}

/// One query for the list and the CSV: each order with its confirmed refunds and its latest refund job.
const ORDERS_SQL: &str = "
    SELECT o.id AS order_id, o.reference, o.amount::text AS amount, o.status, o.path, o.payer, o.created_at, o.paid_at,
           COALESCE((SELECT SUM((j.args->>'amount')::numeric) FROM relayer_jobs j
                     WHERE j.order_id = o.id AND j.kind = 'direct_refund' AND j.status = 'confirmed'), 0)::text AS refunded,
           r.status AS refund_status, r.last_error AS refund_error
    FROM orders o
    LEFT JOIN LATERAL (SELECT status, last_error FROM relayer_jobs
                       WHERE order_id = o.id AND kind = 'direct_refund' AND status <> 'confirmed'
                       ORDER BY id DESC LIMIT 1) r ON TRUE
    WHERE o.merchant_id = $1 AND ($2::text[] IS NULL OR o.status = ANY($2)) AND o.created_at < $3
    ORDER BY o.created_at DESC, o.id
    LIMIT $4";

async fn order_rows(
    state: &AppState,
    merchant_id: i64,
    statuses: Option<Vec<String>>,
    before: i64,
    limit: i64,
) -> Result<Vec<OrderRow>, ApiError> {
    let mut rows: Vec<OrderRow> = sqlx::query_as(ORDERS_SQL)
        .bind(merchant_id)
        .bind(statuses)
        .bind(before)
        .bind(limit)
        .fetch_all(&state.pool)
        .await?;
    for row in &mut rows {
        row.amount = usdt(&row.amount);
        row.refunded = usdt(&row.refunded);
    }
    Ok(rows)
}

pub async fn list_orders(
    State(state): State<AppState>,
    shop: Shop,
    Query(query): Query<OrdersQuery>,
) -> Result<Json<OrdersPage>, ApiError> {
    let statuses = query.status.filter(|text| !text.is_empty()).map(|text| text.split(',').map(str::to_owned).collect());
    let orders = order_rows(&state, shop.merchant_id, statuses, query.before.unwrap_or(i64::MAX), PAGE).await?;
    let next = (orders.len() == usize::try_from(PAGE).unwrap_or(usize::MAX)).then(|| orders.last().map(|o| o.created_at)).flatten();
    Ok(Json(OrdersPage { orders, next }))
}

pub async fn payments_csv(State(state): State<AppState>, shop: Shop) -> Result<impl IntoResponse, ApiError> {
    let rows = order_rows(&state, shop.merchant_id, None, i64::MAX, CSV_ROWS).await?;
    let mut csv = String::from("order_id,reference,amount_usdt,refunded_usdt,status,path,payer,created_at,paid_at\n");
    for row in rows {
        let line = [
            row.order_id,
            row.reference,
            row.amount,
            row.refunded,
            row.status,
            row.path.unwrap_or_default(),
            row.payer.unwrap_or_default(),
            row.created_at.to_string(),
            row.paid_at.map(|at| at.to_string()).unwrap_or_default(),
        ]
        .map(|cell| csv_cell(&cell))
        .join(",");
        csv.push_str(&line);
        csv.push('\n');
    }
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"uniform-payments.csv\""),
        ],
        csv,
    ))
}

/// Quoted when needed, and never read as a formula by a spreadsheet (a reference is shop text).
fn csv_cell(text: &str) -> String {
    let text = if text.starts_with(['=', '+', '-', '@']) { format!("'{text}") } else { text.to_owned() };
    if text.contains([',', '"', '\n', '\r']) { format!("\"{}\"", text.replace('"', "\"\"")) } else { text }
}

fn usdt(units: &str) -> String {
    units.parse::<Amount>().map(|amount| amount.to_decimal(USDT_DECIMALS)).unwrap_or_else(|_| units.to_owned())
}

/* ---------- Settings ---------- */

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    name: String,
    payout_address: String,
    webhook_url: Option<String>,
    webhook_secret: String,
    created_at: i64,
}

async fn settings(state: &AppState, merchant_id: i64) -> Result<Settings, ApiError> {
    Ok(sqlx::query_as("SELECT name, payout_address, webhook_url, webhook_secret, created_at FROM merchants WHERE id = $1")
        .bind(merchant_id)
        .fetch_one(&state.pool)
        .await?)
}

pub async fn read_shop(State(state): State<AppState>, owner: Owner) -> Result<Json<Settings>, ApiError> {
    Ok(Json(settings(&state, owner.merchant_id).await?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShopChanges {
    /// `null` or "" turns webhooks off.
    webhook_url: Option<String>,
}

pub async fn update_shop(
    State(state): State<AppState>,
    owner: Owner,
    body: Result<Json<ShopChanges>, JsonRejection>,
) -> Result<Json<Settings>, ApiError> {
    let Json(body) = body.map_err(|rejection| ApiError::bad_request("bad_json", rejection.body_text()))?;
    let url = body.webhook_url.map(|url| url.trim().to_owned()).filter(|url| !url.is_empty());
    if let Some(url) = &url
        && (!(url.starts_with("https://") || url.starts_with("http://")) || url.len() > MAX_URL_LENGTH)
    {
        return Err(ApiError::bad_request("bad_webhook_url", "the webhook URL must start with https:// (or http://)"));
    }
    sqlx::query("UPDATE merchants SET webhook_url = $2 WHERE id = $1")
        .bind(owner.merchant_id)
        .bind(url)
        .execute(&state.pool)
        .await?;
    Ok(Json(settings(&state, owner.merchant_id).await?))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSecret {
    secret: String,
}

/// A new webhook secret. Webhooks already queued are signed with it when they are sent.
pub async fn rotate_webhook_secret(State(state): State<AppState>, owner: Owner) -> Result<Json<NewSecret>, ApiError> {
    let secret = format!("whsec_{}", hex::encode(random::bytes32()));
    sqlx::query("UPDATE merchants SET webhook_secret = $2 WHERE id = $1")
        .bind(owner.merchant_id)
        .bind(&secret)
        .execute(&state.pool)
        .await?;
    Ok(Json(NewSecret { secret }))
}

/// A new API key, shown once. The old one stops working at once.
pub async fn rotate_api_key(State(state): State<AppState>, owner: Owner) -> Result<Json<NewSecret>, ApiError> {
    let (key, hash) = new_api_key();
    sqlx::query("UPDATE merchants SET api_key_hash = $2 WHERE id = $1")
        .bind(owner.merchant_id)
        .bind(hash)
        .execute(&state.pool)
        .await?;
    Ok(Json(NewSecret { secret: key }))
}

/* ---------- Webhook log ---------- */

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    id: i64,
    order_id: String,
    reference: String,
    event: String,
    url: String,
    status: String,
    attempts: i32,
    last_status: Option<i32>,
    last_error: Option<String>,
    next_attempt_at: i64,
    created_at: i64,
    delivered_at: Option<i64>,
    #[sqlx(json)]
    body: Value,
}

pub async fn list_webhooks(State(state): State<AppState>, owner: Owner) -> Result<Json<Vec<Delivery>>, ApiError> {
    let rows = sqlx::query_as(
        "SELECT w.id, w.order_id, o.reference, w.event, w.url, w.status, w.attempts, w.last_status, w.last_error,
                w.next_attempt_at, w.created_at, w.delivered_at, w.body::json AS body
         FROM webhook_log w JOIN orders o ON o.id = w.order_id
         WHERE w.merchant_id = $1
         ORDER BY w.id DESC
         LIMIT $2",
    )
    .bind(owner.merchant_id)
    .bind(PAGE)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

/// Sends a webhook again on the Notifier's next pass, to the shop's current URL.
pub async fn retry_webhook(
    State(state): State<AppState>,
    owner: Owner,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let updated = sqlx::query(
        "UPDATE webhook_log w SET status = 'pending', attempts = 0, next_attempt_at = $3, url = m.webhook_url
         FROM merchants m
         WHERE w.id = $1 AND w.merchant_id = $2 AND m.id = w.merchant_id AND m.webhook_url IS NOT NULL",
    )
    .bind(id)
    .bind(owner.merchant_id)
    .bind(now())
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::not_found("webhook (or the shop has no webhook URL)"));
    }
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use serde_json::{Value, json};
    use sqlx::PgPool;
    use tower::ServiceExt;

    use super::csv_cell;
    use crate::{
        auth::new_session_token,
        routes::test_support::{TestApp, call, test_app},
    };

    async fn session(app: &TestApp) -> String {
        let (token, hash) = new_session_token();
        sqlx::query("INSERT INTO sessions (token_hash, merchant_id, created_at, expires_at) SELECT $1, id, 1, $2 FROM merchants")
            .bind(hash)
            .bind(i64::MAX)
            .execute(&app.pool)
            .await
            .unwrap();
        token
    }

    fn request(method: &str, path: &str, token: &str, body: Option<Value>) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
            .unwrap()
    }

    #[sqlx::test]
    async fn orders_list_newest_first_and_filter_by_status(pool: PgPool) {
        let app = test_app(pool).await;
        for (amount, reference) in [("1", "first"), ("2.5", "second")] {
            app.create_order(json!({ "amount": amount, "reference": reference })).await;
        }
        sqlx::query("UPDATE orders SET status = 'paid', created_at = created_at - 10 WHERE reference = 'first'")
            .execute(&app.pool)
            .await
            .unwrap();

        let (status, page) = call(&app.app, request("GET", "/v1/orders", &app.key, None)).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let references: Vec<&str> = page["orders"].as_array().unwrap().iter().map(|o| o["reference"].as_str().unwrap()).collect();
        assert_eq!(references, ["second", "first"]);
        assert_eq!(page["orders"][0]["amount"], "2.500000");
        assert_eq!(page["orders"][1]["refunded"], "0.000000");

        let (_, paid) = call(&app.app, request("GET", "/v1/orders?status=paid,refunded", &app.key, None)).await;
        assert_eq!(paid["orders"].as_array().unwrap().len(), 1);
    }

    #[sqlx::test]
    async fn settings_need_a_session_not_the_api_key(pool: PgPool) {
        let app = test_app(pool).await;
        assert_eq!(call(&app.app, request("GET", "/v1/shop", &app.key, None)).await.0, StatusCode::UNAUTHORIZED);

        let token = session(&app).await;
        let change = json!({ "webhookUrl": "https://shop.example/hook" });
        let (status, shop) = call(&app.app, request("PATCH", "/v1/shop", &token, Some(change))).await;
        assert_eq!(status, StatusCode::OK, "{shop}");
        assert_eq!(shop["webhookUrl"], "https://shop.example/hook");
        let bad = json!({ "webhookUrl": "ftp://shop.example" });
        assert_eq!(call(&app.app, request("PATCH", "/v1/shop", &token, Some(bad))).await.0, StatusCode::BAD_REQUEST);

        let (_, rotated) = call(&app.app, request("POST", "/v1/shop/api-key", &token, None)).await;
        let new_key = rotated["secret"].as_str().unwrap();
        assert_eq!(call(&app.app, request("GET", "/v1/orders", &app.key, None)).await.0, StatusCode::UNAUTHORIZED, "old key");
        assert_eq!(call(&app.app, request("GET", "/v1/orders", new_key, None)).await.0, StatusCode::OK);
    }

    #[sqlx::test]
    async fn the_csv_has_a_header_and_a_row_per_order(pool: PgPool) {
        let app = test_app(pool).await;
        app.create_order(json!({ "amount": "3", "reference": "=cmd, \"x\"" })).await;
        let response = app.app.clone().oneshot(request("GET", "/v1/payments.csv", &app.key, None)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let text = String::from_utf8(to_bytes(response.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("order_id,reference,amount_usdt"));
        assert!(lines[1].contains(",\"'=cmd, \"\"x\"\"\",3.000000,0.000000,created,"), "{}", lines[1]);
    }

    #[test]
    fn csv_cells_are_quoted_and_never_formulas() {
        assert_eq!(csv_cell("plain"), "plain");
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("=1+1"), "'=1+1");
    }
}
