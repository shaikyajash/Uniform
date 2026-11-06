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
  tokens: Record<string, { address: string; decimals: number }>;
};

// shared/chains.json is the one list of chains and addresses. Turbopack cannot import files
// outside web/, so it is read here and handed to the browser as one public value.
const chains: Record<string, unknown> = JSON.parse(readFileSync(path.join(repo, "shared/chains.json"), "utf8"));
const shop = Object.values(chains).find(
  (entry): entry is ChainEntry => typeof entry === "object" && entry !== null && (entry as ChainEntry).role === "shop",
);
if (!shop) throw new Error("shared/chains.json has no chain with role \"shop\"");

const shopChain = {
  id: shop.chainId,
  rpc: shop.rpc,
  explorer: shop.explorer,
  usdt: shop.tokens.USDT.address,
  usdtDecimals: shop.tokens.USDT.decimals,
};

const orderDesk = fromRepoEnv("ORDER_DESK_URL") ?? "http://localhost:4000";

const nextConfig: NextConfig = {
  cacheComponents: true,
  partialPrefetching: true,
  env: {
    NEXT_PUBLIC_PRIVY_APP_ID: fromRepoEnv("NEXT_PUBLIC_PRIVY_APP_ID") ?? "",
    NEXT_PUBLIC_SHOP_CHAIN: JSON.stringify(shopChain),
  },
  // The pay page calls the Order Desk through this same site, so the browser never makes a
  // cross-site request and the Order Desk needs no CORS. Only the public /v1 API is forwarded.
  async rewrites() {
    return [{ source: "/desk/v1/:path*", destination: `${orderDesk}/v1/:path*` }];
  },
};

export default nextConfig;
