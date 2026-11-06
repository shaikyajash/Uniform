//! Contract bindings generated from `shared/abis/`. Owned by A.
//!
//! Each module exposes the contract's calls, events and structs, plus an `rpc` instance type
//! (`new(address, provider)`) for reading state and sending transactions.

#![allow(clippy::too_many_arguments)]

alloy::sol!(
    #[sol(rpc)]
    #[derive(Debug)]
    DirectPay,
    "../../shared/abis/DirectPay.json"
);

alloy::sol!(
    #[sol(rpc)]
    #[derive(Debug)]
    SourceLock,
    "../../shared/abis/SourceLock.json"
);

alloy::sol!(
    #[sol(rpc)]
    #[derive(Debug)]
    PayoutPool,
    "../../shared/abis/PayoutPool.json"
);

alloy::sol!(
    #[sol(rpc)]
    #[derive(Debug)]
    DestinationLock,
    "../../shared/abis/DestinationLock.json"
);
