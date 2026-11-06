//! `order-desk add-shop`: registers a shop and prints its API key and webhook secret once.
//!
//! Stands in for the Shop Dashboard (B7) until it exists, and sets up the demo shop on Railway.

use alloy::{hex, primitives::Address};
use sqlx::PgPool;

use crate::{auth::new_api_key, random, routes::now};

pub struct NewShop {
    pub id: i64,
    pub api_key: String,
    pub webhook_secret: String,
}

pub async fn add_shop(pool: &PgPool, name: &str, payout: Address) -> Result<NewShop, sqlx::Error> {
    let (api_key, api_key_hash) = new_api_key();
    let webhook_secret = format!("whsec_{}", hex::encode(random::bytes32()));
    let id = sqlx::query_scalar(
        "INSERT INTO merchants (name, payout_address, api_key_hash, webhook_secret, created_at)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(name)
    // Lowercase hex, the form the database's `address` type accepts.
    .bind(format!("{payout:#x}"))
    .bind(api_key_hash)
    .bind(&webhook_secret)
    .bind(now())
    .fetch_one(pool)
    .await?;
    Ok(NewShop { id, api_key, webhook_secret })
}

/// `add-shop --name "Kettle & Co" --payout 0x...`
pub async fn add_shop_command(pool: &PgPool, args: &[String]) -> Result<(), String> {
    let usage = "usage: order-desk add-shop --name \"Kettle & Co\" --payout 0x<shop wallet>";
    let mut name = None;
    let mut payout = None;
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(usage)?;
        match flag.as_str() {
            "--name" => name = Some(value.trim().to_owned()),
            "--payout" => payout = Some(value.parse::<Address>().map_err(|error| format!("--payout: {error}"))?),
            _ => return Err(usage.to_owned()),
        }
    }
    let (Some(name), Some(payout)) = (name.filter(|name| !name.is_empty()), payout) else {
        return Err(usage.to_owned());
    };

    let shop = add_shop(pool, &name, payout).await.map_err(|error| match error {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            format!("a shop with payout address {payout:#x} already exists")
        }
        other => other.to_string(),
    })?;
    println!("Shop {} \"{name}\" added, paid at {payout:#x}.", shop.id);
    println!("Shown once, store them now:");
    println!("  API key:        {}", shop.api_key);
    println!("  Webhook secret: {}", shop.webhook_secret);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    use crate::auth::hash_api_key;

    #[sqlx::test]
    async fn a_new_shop_stores_only_the_key_hash_and_a_lowercase_address(pool: PgPool) {
        let payout = address!("5A30000000000000000000000000000000000ABC");
        let shop = add_shop(&pool, "Kettle & Co", payout).await.unwrap();
        let (address, hash, secret): (String, String, String) =
            sqlx::query_as("SELECT payout_address, api_key_hash, webhook_secret FROM merchants WHERE id = $1")
                .bind(shop.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(address, "0x5a30000000000000000000000000000000000abc");
        assert_eq!(hash, hash_api_key(&shop.api_key));
        assert_eq!(secret, shop.webhook_secret);
        assert!(shop.webhook_secret.starts_with("whsec_"));
    }

    #[sqlx::test]
    async fn the_same_payout_address_cannot_be_two_shops(pool: PgPool) {
        let payout = address!("5a30000000000000000000000000000000000001");
        add_shop(&pool, "First", payout).await.unwrap();
        let args = ["--name", "Second", "--payout", "0x5a30000000000000000000000000000000000001"].map(String::from);
        let error = add_shop_command(&pool, &args).await.unwrap_err();
        assert!(error.contains("already exists"), "{error}");
    }

    #[sqlx::test]
    async fn missing_or_bad_flags_show_the_usage(pool: PgPool) {
        for args in [vec!["--name", "X"], vec!["--payout", "0x12", "--name", "X"], vec!["--color", "red"]] {
            let args: Vec<String> = args.into_iter().map(String::from).collect();
            assert!(add_shop_command(&pool, &args).await.is_err(), "{args:?}");
        }
    }
}
