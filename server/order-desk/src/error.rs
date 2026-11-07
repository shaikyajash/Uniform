//! The one error type every route returns.
//!
//! The client always gets `{"error": {"code": "...", "message": "..."}}` with a matching HTTP
//! status. `code` is stable for programs to match on; `message` is for people.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ApiError { status, code, message: message.into() }
    }

    /// 400: the request itself is wrong (bad JSON field, bad amount, ...).
    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    /// 401: missing or wrong API key or token.
    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid credentials")
    }

    /// 404: `what` does not exist, or belongs to another shop.
    pub fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", format!("{what} not found"))
    }

    /// 409: the request is valid but the order is not in a state that allows it.
    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    /// 500: our fault. The details go to the server log, never to the client.
    pub fn internal(detail: impl std::fmt::Display) -> Self {
        eprintln!("order-desk: internal error: {detail}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "something went wrong on our side")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "error": { "code": self.code, "message": self.message } });
        (self.status, Json(body)).into_response()
    }
}

/// A database failure is never the client's fault, and its text can contain table details.
impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        ApiError::internal(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::Value;

    async fn reply(error: ApiError) -> (StatusCode, Value) {
        let response = error.into_response();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn the_body_has_code_and_message() {
        let (status, body) = reply(ApiError::conflict("order_not_payable", "order is already paid")).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body, json!({ "error": { "code": "order_not_payable", "message": "order is already paid" } }));
    }

    #[tokio::test]
    async fn database_errors_do_not_leak_details() {
        let (status, body) = reply(ApiError::from(sqlx::Error::PoolTimedOut)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"]["code"], "internal");
        assert!(!body.to_string().to_lowercase().contains("pool"), "{body}");
    }

    #[tokio::test]
    async fn helpers_pick_the_right_status() {
        assert_eq!(reply(ApiError::bad_request("bad_amount", "x")).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(reply(ApiError::unauthorized()).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(reply(ApiError::not_found("order")).await.1["error"]["message"], "order not found");
    }
}
