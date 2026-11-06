//! Jobs the Order Desk hands to the Relayer through the `relayer_jobs` table.
//!
//! A job is typed arguments, never raw calldata: the Relayer builds each call itself, so it can
//! only send the calls listed here. That is its drain protection.

use alloy::primitives::{Address, B256, Bytes};
use serde::{Deserialize, Serialize};

use crate::amount::Amount;

/// What to send. Stored as JSON in `relayer_jobs.args`; `kind` is also its own column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Job {
    /// Path A: `DirectPay.pay(orderId, merchant, amount, payer, validAfter, validBefore, v, r, s)`
    /// on the shop chain, from the note the customer signed.
    DirectPay {
        order_id: B256,
        merchant: Address,
        amount: Amount,
        payer: Address,
        valid_after: u64,
        valid_before: u64,
        /// The EIP-3009 nonce in the note (`paymentIdFor`), to check `authorizationState`.
        nonce: B256,
        /// 65 bytes: r, s, v.
        signature: Bytes,
    },
}

impl Job {
    /// The value of `relayer_jobs.kind`.
    pub const fn kind(&self) -> &'static str {
        match self {
            Job::DirectPay { .. } => "direct_pay",
        }
    }
}

/// Where a job is. Stored in `relayer_jobs.status`.
///
/// `pending -> sending -> sent -> confirmed`, or `failed` from any step before `confirmed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// Written by the Order Desk, not taken yet.
    Pending,
    /// Checked and signed; the signed transaction and its hash are saved, not yet accepted by a
    /// node. Retries send the same bytes.
    Sending,
    /// A node accepted it; waiting for the receipt.
    Sent,
    /// Mined and succeeded.
    Confirmed,
    /// A check, the dry run or the transaction failed; `last_error` says why.
    Failed,
}

impl JobStatus {
    pub const ALL: [JobStatus; 5] =
        [JobStatus::Pending, JobStatus::Sending, JobStatus::Sent, JobStatus::Confirmed, JobStatus::Failed];

    pub const fn as_str(self) -> &'static str {
        match self {
            JobStatus::Pending => "pending",
            JobStatus::Sending => "sending",
            JobStatus::Sent => "sent",
            JobStatus::Confirmed => "confirmed",
            JobStatus::Failed => "failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;
    use serde_json::json;

    fn direct_pay() -> Job {
        Job::DirectPay {
            order_id: B256::repeat_byte(0xab),
            merchant: address!("5a30000000000000000000000000000000000001"),
            amount: Amount::from(12_000_000u64),
            payer: address!("a917000000000000000000000000000000000001"),
            valid_after: 1_793_913_940,
            valid_before: 1_793_914_600,
            nonce: B256::repeat_byte(0xdc),
            signature: Bytes::from(vec![7u8; 65]),
        }
    }

    #[test]
    fn json_names_the_kind_and_round_trips() {
        let value = serde_json::to_value(direct_pay()).unwrap();
        assert_eq!(value["kind"], "direct_pay");
        assert_eq!(value["amount"], "12000000");
        assert_eq!(value["validBefore"], 1_793_914_600);
        assert_eq!(value["signature"], json!(format!("0x{}", "07".repeat(65))));
        assert_eq!(serde_json::from_value::<Job>(value).unwrap(), direct_pay());
        assert_eq!(direct_pay().kind(), "direct_pay");
    }

    #[test]
    fn unknown_kinds_are_rejected() {
        let mut value = serde_json::to_value(direct_pay()).unwrap();
        value["kind"] = json!("transfer_everything");
        assert!(serde_json::from_value::<Job>(value).is_err());
    }

    #[test]
    fn status_text_matches_serde() {
        for status in JobStatus::ALL {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(status.as_str()));
        }
    }
}
