// The shop chain (Whitechain), from shared/chains.json via next.config.ts.

import { createPublicClient, defineChain, erc20Abi, formatUnits, http, type Address } from "viem";

type ShopChain = { id: number; rpc: string; explorer: string; usdt: Address; usdtDecimals: number };

export const shopChain: ShopChain = JSON.parse(process.env.NEXT_PUBLIC_SHOP_CHAIN ?? "null");

export const shopViemChain = defineChain({
  id: shopChain.id,
  name: "Whitechain Testnet",
  nativeCurrency: { name: "WhiteBIT Coin", symbol: "WBT", decimals: 18 },
  rpcUrls: { default: { http: [shopChain.rpc] } },
  blockExplorers: { default: { name: "Whitechain Explorer", url: shopChain.explorer } },
  testnet: true,
});

const client = createPublicClient({ chain: shopViemChain, transport: http(shopChain.rpc) });

/** The wallet's USDT on the shop chain, in base units. Read straight from the chain. */
export async function usdtBalance(owner: Address): Promise<bigint> {
  return client.readContract({ address: shopChain.usdt, abi: erc20Abi, functionName: "balanceOf", args: [owner] });
}

/** "12.5" from 12_500_000 base units, at least 2 decimals. */
export function formatUsdt(units: bigint): string {
  return trimAmount(formatUnits(units, shopChain.usdtDecimals));
}

/** "12.500000" -> "12.50"; "0.123400" -> "0.1234". */
export function trimAmount(decimal: string): string {
  const [whole, fraction = ""] = decimal.split(".");
  const trimmed = fraction.replace(/0+$/, "").padEnd(2, "0");
  return `${whole}.${trimmed}`;
}

export function shortAddress(address: string): string {
  return `${address.slice(0, 6)}…${address.slice(-4)}`;
}
