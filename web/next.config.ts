import type { NextConfig } from "next";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

// `next dev` and `next build` run in web/, so the repo is one level up.
const repo = path.resolve(process.cwd(), "..");

/**
 * Reads one key from the repo's `.env` (or the real environment, which wins).
 * Only the keys named below are read, so server secrets in that file never reach the web app.
 */
function fromRepoEnv(key: string): string | undefined {
  if (process.env[key]) return process.env[key];
  const file = path.join(repo, ".env");
  if (!existsSync(file)) return undefined;
  for (const line of readFileSync(file, "utf8").split("\n")) {
    const match = line.match(/^\s*([A-Z0-9_]+)\s*=\s*(.*?)\s*$/);
    if (match?.[1] === key) return match[2].replace(/^["']|["']$/g, "");
  }
  return undefined;
}

type ChainEntry = {
  chainId: number;
  role: string;
  rpc: string;
  explorer: string;
  tokens: Record<string, { address: string; decimals: number; priceId?: string }>;
  contracts: Record<string, string>;
};

// shared/chains.json is the one list of chains and addresses. Turbopack cannot import files
// outside web/, so it is read here and handed to the browser as one public value.
const chains: Record<string, unknown> = JSON.parse(readFileSync(path.join(repo, "shared/chains.json"), "utf8"));
const entries = Object.entries(chains).filter(
  (pair): pair is [string, ChainEntry] => typeof pair[1] === "object" && pair[1] !== null && "chainId" in pair[1],
);
const shop = entries.find(([, entry]) => entry.role === "shop")?.[1];
if (!shop) throw new Error("shared/chains.json has no chain with role \"shop\"");

const shopChain = {
  id: shop.chainId,
  rpc: shop.rpc,
  explorer: shop.explorer,
  usdt: shop.tokens.USDT.address,
  usdtDecimals: shop.tokens.USDT.decimals,
  destinationLock: shop.contracts.destinationLock || null,
};

// Customer chains a payment can come from: those whose SourceLock is deployed and that list USDC.
// `tokens` are the priced ERC-20s (WETH, WBTC, ...) paid through a Calibur batch; `calibur` is the
// EIP-7702 delegate that runs it (null where it is not deployed, and then those tokens are skipped).
const customerChains = entries
  .filter(([, entry]) => entry.role === "customer" && entry.contracts.sourceLock && entry.tokens.USDC)
  .map(([key, entry]) => ({
    key,
    id: entry.chainId,
    rpc: entry.rpc,
    explorer: entry.explorer,
    usdc: entry.tokens.USDC.address,
    usdcDecimals: entry.tokens.USDC.decimals,
    sourceLock: entry.contracts.sourceLock,
    calibur: entry.contracts.calibur || null,
    tokens: entry.contracts.calibur
      ? Object.entries(entry.tokens)
          .filter(([, token]) => token.priceId)
          .map(([symbol, token]) => ({ symbol, address: token.address, decimals: token.decimals, priceId: token.priceId }))
      : [],
  }));

const orderDesk = fromRepoEnv("ORDER_DESK_URL") ?? "http://localhost:4000";

// Server-only settings for the Demo Shop's own routes (/demo/api/*). They go into this server
// process's environment, never into `env` below, so the browser never sees them.
process.env.ORDER_DESK_URL ??= orderDesk;
for (const key of ["DEMO_SHOP_API_KEY", "DEMO_WEBHOOK_SECRET"]) {
  const value = fromRepoEnv(key);
  if (value) process.env[key] ??= value;
}

const nextConfig: NextConfig = {
  // Extra hosts allowed to load the dev server, such as this machine's address on the local
  // network for trying the pay page on a phone: DEV_ORIGINS=10.0.0.5,other-host.
  allowedDevOrigins: process.env.DEV_ORIGINS?.split(",").map((host) => host.trim()).filter(Boolean),
  cacheComponents: true,
  partialPrefetching: true,
  env: {
    NEXT_PUBLIC_PRIVY_APP_ID: fromRepoEnv("NEXT_PUBLIC_PRIVY_APP_ID") ?? "",
    NEXT_PUBLIC_SHOP_CHAIN: JSON.stringify(shopChain),
    NEXT_PUBLIC_CUSTOMER_CHAINS: JSON.stringify(customerChains),
    NEXT_PUBLIC_PROVIDER_FEE_BPS: String(chains.providerFeeBps ?? 30),
    NEXT_PUBLIC_TIMEOUTS: JSON.stringify(chains.timeoutsSeconds ?? {}),
  },
  // The pay page calls the Order Desk through this same site, so the browser never makes a
  // cross-site request and the Order Desk needs no CORS. Only the public /v1 API is forwarded.
  async rewrites() {
    return [{ source: "/desk/v1/:path*", destination: `${orderDesk}/v1/:path*` }];
  },
};

export default nextConfig;
