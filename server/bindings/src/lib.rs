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

alloy::sol!(
    /// The parts of Circle's FiatToken (USDC, EURC) the server reads.
    #[sol(rpc)]
    interface IFiatToken {
        function isBlacklisted(address account) external view returns (bool);
    }
);

alloy::sol!(
    /// The parts of 1inch Aqua the server reads.
    #[sol(rpc)]
    interface IAqua {
        function rawBalances(address maker, address app, bytes32 strategyHash, address token)
            external view returns (uint248 balance, uint8 tokensCount);
    }
);
