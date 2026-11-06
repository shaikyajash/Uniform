// The customer's secret for a cross-chain payment (SPEC, "Customer secret").
//
// The wallet signs a fixed message for this order; the secret is keccak256 of that signature and
// the hashlock is keccak256 of the secret. The same order gives the same signature, so a closed
// tab gets the same secret back by signing again. Nothing is stored anywhere: not in the browser,
// not on the server. The secret leaves the page only when it is revealed to pay the merchant.

import { keccak256, type Hex } from "viem";
import { shopChain } from "./chain";

export function secretTypedData(orderId: Hex, amountUnits: bigint) {
  return {
    domain: { name: "Uniform", version: "1", chainId: shopChain.id },
    types: {
      PaymentKey: [
        { name: "orderId", type: "bytes32" },
        { name: "amount", type: "uint256" },
      ],
    },
    primaryType: "PaymentKey" as const,
    message: { orderId, amount: amountUnits.toString() },
  };
}

export function secretFromSignature(signature: Hex): { secret: Hex; hashlock: Hex } {
  const secret = keccak256(signature);
  return { secret, hashlock: keccak256(secret) };
}
