//! Loader for `shared/chains.json`: chains, tokens, contract addresses, timeouts and the fee.
//!
//! Unknown keys are ignored, so A can add fields without breaking the server. A contract that is
//! not deployed yet is `""` in the file and `None` here.

use std::{collections::BTreeMap, fmt, fs, io, path::Path};

use alloy::primitives::Address;
use serde::{Deserialize, Deserializer};

/// The whole file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chains {
    pub timeouts_seconds: Timeouts,
    /// Bob's fee in basis points, added to the customer's quote (30 = 0.3%).
    pub provider_fee_bps: u16,
    /// Every other top-level key is a chain, keyed by its name (`whitechainSepolia`, ...).
    #[serde(flatten)]
    pub by_name: BTreeMap<String, Chain>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chain {
    pub chain_id: u64,
    pub role: Role,
    pub rpc: String,
    pub explorer: String,
    /// Keyed by symbol: `USDT`, `USDC`, `EURC`.
    pub tokens: BTreeMap<String, Token>,
    pub permit2: Address,
    pub contracts: Contracts,
    #[serde(default)]
    pub uniswap: Option<Uniswap>,
    /// Blocks to wait before trusting an event on this chain.
    pub confirmations: u64,
}

/// Whether the shop is paid on this chain, or customers pay from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Shop,
    Customer,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Token {
    pub address: Address,
    pub decimals: u8,
    /// The token's EIP-712 domain, for notes customers sign (EIP-3009). Hard-coded because
    /// Whitechain USDT has no `version()`; each value was checked against `DOMAIN_SEPARATOR()`.
    pub eip712: Eip712Domain,
}

/// The `name` and `version` of a token's EIP-712 domain. Chain id and contract come from the token.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Eip712Domain {
    pub name: String,
    pub version: String,
}

/// Our contracts on one chain. Each is `None` until deployed there.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Contracts {
    #[serde(deserialize_with = "optional_address")]
    pub aqua: Option<Address>,
    #[serde(deserialize_with = "optional_address")]
    pub direct_pay: Option<Address>,
    #[serde(deserialize_with = "optional_address")]
    pub payout_pool: Option<Address>,
    /// `shopLock` is the name on `dev` before A's rename lands; remove the alias after it does.
    #[serde(deserialize_with = "optional_address", alias = "shopLock")]
    pub destination_lock: Option<Address>,
    #[serde(deserialize_with = "optional_address")]
    pub source_lock: Option<Address>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Uniswap {
    #[serde(deserialize_with = "optional_address")]
    pub pool_manager: Option<Address>,
    #[serde(deserialize_with = "optional_address")]
    pub universal_router: Option<Address>,
    #[serde(deserialize_with = "optional_address")]
    pub position_manager: Option<Address>,
    #[serde(deserialize_with = "optional_address")]
    pub quoter: Option<Address>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timeouts {
    pub source_lock: u64,
    /// `shopLock` is the name on `dev` before A's rename lands; remove the alias after it does.
    #[serde(alias = "shopLock")]
    pub destination_lock: u64,
    /// The Matchmaker fills only if the source lock has at least this long left.
    pub fill_cutoff: u64,
    pub quote: u64,
}

/// `""` means "not deployed yet".
fn optional_address<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Address>, D::Error> {
    let text = String::deserialize(deserializer)?;
    if text.is_empty() {
        return Ok(None);
    }
    text.parse().map(Some).map_err(serde::de::Error::custom)
}

#[derive(Debug)]
pub enum LoadError {
    Read { path: String, source: io::Error },
    Parse(serde_json::Error),
    /// The file parsed but breaks a rule the server relies on.
    Invalid(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Read { path, source } => write!(f, "cannot read {path}: {source}"),
            LoadError::Parse(error) => write!(f, "chains.json is not valid: {error}"),
            LoadError::Invalid(reason) => write!(f, "chains.json breaks a rule: {reason}"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoadError::Read { source, .. } => Some(source),
            LoadError::Parse(error) => Some(error),
            LoadError::Invalid(_) => None,
        }
    }
}

impl Chains {
    pub fn load(path: impl AsRef<Path>) -> Result<Chains, LoadError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| LoadError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Chains::from_json(&text)
    }

    pub fn from_json(text: &str) -> Result<Chains, LoadError> {
        let chains: Chains = serde_json::from_str(text).map_err(LoadError::Parse)?;
        chains.check()?;
        Ok(chains)
    }

    pub fn by_id(&self, chain_id: u64) -> Option<&Chain> {
        self.by_name.values().find(|chain| chain.chain_id == chain_id)
    }

    /// The one chain where shops are paid (Whitechain).
    pub fn shop_chain(&self) -> &Chain {
        self.by_name
            .values()
            .find(|chain| chain.role == Role::Shop)
            .expect("check() guarantees one shop chain")
    }

    pub fn customer_chains(&self) -> impl Iterator<Item = &Chain> {
        self.by_name.values().filter(|chain| chain.role == Role::Customer)
    }

    fn check(&self) -> Result<(), LoadError> {
        let shop_chains = self.by_name.values().filter(|chain| chain.role == Role::Shop).count();
        if shop_chains != 1 {
            return Err(LoadError::Invalid(format!("expected 1 chain with role \"shop\", found {shop_chains}")));
        }
        let mut seen = BTreeMap::new();
        for (name, chain) in &self.by_name {
            if let Some(other) = seen.insert(chain.chain_id, name) {
                return Err(LoadError::Invalid(format!("{other} and {name} share chain id {}", chain.chain_id)));
            }
            if chain.confirmations == 0 {
                return Err(LoadError::Invalid(format!("{name}: confirmations must be at least 1")));
            }
        }
        let timeouts = self.timeouts_seconds;
        if timeouts.destination_lock >= timeouts.source_lock {
            return Err(LoadError::Invalid(
                "the destination lock must expire before the source lock".to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    /// The real file in the repo, so a change to it that breaks the server fails here.
    const REPO_FILE: &str = include_str!("../../../shared/chains.json");

    #[test]
    fn the_repo_file_loads() {
        let chains = Chains::from_json(REPO_FILE).unwrap();

        let shop = chains.shop_chain();
        assert_eq!(shop.chain_id, 1874);
        assert_eq!(shop.confirmations, 1);
        assert_eq!(shop.tokens["USDT"].address, address!("cedfB56113143d55b2358b8CFbB0dEB5Ad2f55a7"));
        assert_eq!(shop.contracts.direct_pay, Some(address!("9DF1C7C32Ce3f1322e1c7bAEc30664b1c472DaE9")));

        let base = chains.by_id(84532).unwrap();
        assert_eq!(base.role, Role::Customer);
        assert_eq!(base.contracts.source_lock, Some(address!("02dF2659B5B4956EAFEe3293584C2aaefC114871")));

        assert_eq!(chains.customer_chains().count(), 2);
        assert_eq!(chains.timeouts_seconds.source_lock, 1200);
        assert_eq!(chains.timeouts_seconds.destination_lock, 600);
        assert_eq!(chains.provider_fee_bps, 30);
    }

    /// Checked on chain on Fri Nov 6: each pair recomputes the token's `DOMAIN_SEPARATOR()`.
    #[test]
    fn tokens_carry_their_checked_eip712_domain() {
        let chains = Chains::from_json(REPO_FILE).unwrap();
        let domain = |chain_id: u64, symbol: &str| chains.by_id(chain_id).unwrap().tokens[symbol].eip712.clone();
        let expect = |name: &str, version: &str| Eip712Domain { name: name.into(), version: version.into() };
        assert_eq!(domain(1874, "USDT"), expect("Tether USD", "1"));
        assert_eq!(domain(84532, "USDC"), expect("USDC", "2"));
        assert_eq!(domain(84532, "EURC"), expect("EURC", "2"));
        assert_eq!(domain(11_155_111, "USDC"), expect("USDC", "2"));
        assert_eq!(domain(11_155_111, "EURC"), expect("EURC", "2"));
    }

    #[test]
    fn not_deployed_is_none() {
        let chains = Chains::from_json(REPO_FILE).unwrap();
        // Ethereum Sepolia's SourceLock is "" in the file.
        assert_eq!(chains.by_id(11_155_111).unwrap().contracts.source_lock, None);
    }

    #[test]
    fn old_and_new_lock_names_both_load() {
        let new_names = REPO_FILE.replace("\"shopLock\"", "\"destinationLock\"");
        let old_names = REPO_FILE.replace("\"destinationLock\"", "\"shopLock\"");
        for text in [new_names, old_names] {
            assert_eq!(Chains::from_json(&text).unwrap().timeouts_seconds.destination_lock, 600);
        }
    }

    #[test]
    fn broken_files_are_rejected_with_a_reason() {
        let two_shops = REPO_FILE.replace("\"role\": \"customer\"", "\"role\": \"shop\"");
        assert!(matches!(Chains::from_json(&two_shops), Err(LoadError::Invalid(_))));

        let bad_address = REPO_FILE.replace("0x9DF1C7C32Ce3f1322e1c7bAEc30664b1c472DaE9", "0x9DF1");
        assert!(matches!(Chains::from_json(&bad_address), Err(LoadError::Parse(_))));

        let wrong_order = REPO_FILE.replace("\"sourceLock\": 1200", "\"sourceLock\": 300");
        assert!(matches!(Chains::from_json(&wrong_order), Err(LoadError::Invalid(_))));
    }
}
