//! Jobs the Order Desk hands to the Relayer through the `relayer_jobs` table.
//!
//! A job is typed arguments, never raw calldata: the Relayer builds each call itself, so it can
//! only send the calls listed here. That is its drain protection.

use alloy::primitives::{Address, B256, Bytes};
use serde::{Deserialize, Serialize};

use crate::{amount::Amount, locks::SourceLockParams, status::UnknownValue};

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
    /// Path B, step 1: `SourceLock.lockWithAuthorization(params, payer, token, amount, validAfter,
    /// validBefore, v, r, s, "")` on a customer chain, from the note the customer signed.
    SourceLock {
        params: SourceLockParams,
        payer: Address,
        token: Address,
        amount: Amount,
        valid_after: u64,
        valid_before: u64,
        /// `lockIdFor(params, payer, token, amount)`: the note's nonce. The Relayer recomputes it.
        lock_id: B256,
        /// 65 bytes: r, s, v.
        signature: Bytes,
    },
    /// Path B, step 3: `DestinationLock.claim(lockId, secret)` on the shop chain. The secret pays
    /// the shop, so the Relayer sends it only to a lock that pays `merchant` at least `min_amount`.
    DestinationClaim {
        lock_id: B256,
        secret: B256,
        merchant: Address,
        min_amount: Amount,
        /// The source lock's expiry: the destination lock always ends before it.
        deadline: u64,
    },
    /// Path B, step 6: `SourceLock.refund(lockId)` once it has timed out: the customer's tokens go
    /// back to them. For locks Bob never filled; the Matchmaker refunds the ones he did.
    SourceRefund { lock_id: B256 },
}

impl Job {
    /// The value of `relayer_jobs.kind`.
    pub const fn kind(&self) -> &'static str {
        match self {
            Job::DirectPay { .. } => "direct_pay",
            Job::SourceLock { .. } => "source_lock",
            Job::DestinationClaim { .. } => "destination_claim",
            Job::SourceRefund { .. } => "source_refund",
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

impl std::str::FromStr for JobStatus {
    type Err = UnknownValue;

    fn from_str(text: &str) -> Result<JobStatus, UnknownValue> {
        JobStatus::ALL.into_iter().find(|status| status.as_str() == text).ok_or_else(|| UnknownValue(text.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;
    use serde_json::json;

    #[test]
    fn job_statuses_read_back_from_their_text() {
        for status in JobStatus::ALL {
            assert_eq!(status.as_str().parse::<JobStatus>(), Ok(status));
        }
        assert!("lost".parse::<JobStatus>().is_err());
    }

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
    fn every_kind_round_trips_with_its_name() {
        let params = SourceLockParams {
            order_id: B256::repeat_byte(0xab),
            receiver: address!("b0b0000000000000000000000000000000000001"),
            min_usdc_out: Amount::from(12_036_000u64),
            hashlock: B256::repeat_byte(0x42),
            expiry: 1_793_915_140,
            out_token: Address::ZERO,
        };
        let jobs = [
            Job::SourceLock {
                params,
                payer: address!("a917000000000000000000000000000000000001"),
                token: address!("036CbD53842c5426634e7929541eC2318f3dCF7e"),
                amount: Amount::from(12_036_000u64),
                valid_after: 1_793_913_940,
                valid_before: 1_793_914_240,
                lock_id: B256::repeat_byte(0x11),
                signature: Bytes::from(vec![7u8; 65]),
            },
            Job::DestinationClaim {
                lock_id: B256::repeat_byte(0x22),
                secret: B256::repeat_byte(0x33),
                merchant: address!("5a30000000000000000000000000000000000001"),
                min_amount: Amount::from(12_000_000u64),
                deadline: 1_793_915_140,
            },
            Job::SourceRefund { lock_id: B256::repeat_byte(0x44) },
        ];
        for job in jobs {
            let value = serde_json::to_value(&job).unwrap();
            assert_eq!(value["kind"], job.kind());
            assert_eq!(serde_json::from_value::<Job>(value).unwrap(), job);
        }
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
