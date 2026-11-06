// The chains, from shared/chains.json via next.config.ts: the merchant chain (Whitechain, where merchants
// are paid in USDT) and the customer chains a payment can come from (USDC, through a SourceLock).
// Everything here reads the chains directly, so the pay page never has to trust the Order Desk
// about money.

import {
  createPublicClient,
  defineChain,
  encodeAbiParameters,
  erc20Abi,
  formatUnits,
  http,
  keccak256,
  parseAbi,
  toBytes,
  type Address,
  type Hex,
  type PublicClient,
} from "viem";

type ShopChain = {
  id: number;
  rpc: string;
  explorer: string;
  usdt: Address;
  usdtDecimals: number;
  destinationLock: Address | null;
};

/** An ERC-20 paid at market price through a Calibur batch (EIP-7702), e.g. WETH or WBTC. */
export type PricedToken = {
  symbol: string;
  address: Address;
  decimals: number;
  /** CoinGecko id, e.g. "ethereum". */
  priceId: string;
};

export type CustomerChain = {
  key: string;
  id: number;
  rpc: string;
  explorer: string;
  usdc: Address;
  usdcDecimals: number;
  sourceLock: Address;
  /** Calibur, the EIP-7702 delegate that runs a priced token's batch. */
  calibur: Address | null;
  tokens: PricedToken[];
};

export const shopChain: ShopChain = JSON.parse(process.env.NEXT_PUBLIC_SHOP_CHAIN ?? "null");
export const customerChains: CustomerChain[] = JSON.parse(process.env.NEXT_PUBLIC_CUSTOMER_CHAINS ?? "[]");
/** Bob's fee on a cross-chain payment, in basis points (30 = 0.3%). */
export const providerFeeBps = Number(process.env.NEXT_PUBLIC_PROVIDER_FEE_BPS ?? "30");

const CHAIN_NAMES: Record<number, string> = {
  1874: "Whitechain",
  84532: "Base Sepolia",
  11155111: "Ethereum Sepolia",
};

export function chainName(id: number): string {
  return CHAIN_NAMES[id] ?? `Chain ${id}`;
}

export const shopViemChain = defineChain({
  id: shopChain.id,
  name: "Whitechain Testnet",
  nativeCurrency: { name: "WhiteBIT Coin", symbol: "WBT", decimals: 18 },
  rpcUrls: { default: { http: [shopChain.rpc] } },
  blockExplorers: { default: { name: "Whitechain Explorer", url: shopChain.explorer } },
  testnet: true,
});

export const customerViemChains = customerChains.map((chain) =>
  defineChain({
    id: chain.id,
    name: chainName(chain.id),
    nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
    rpcUrls: { default: { http: [chain.rpc] } },
    blockExplorers: { default: { name: "Explorer", url: chain.explorer } },
    testnet: true,
  }),
);

const shopClient = createPublicClient({ chain: shopViemChain, transport: http(shopChain.rpc) });
const customerClients = new Map<number, PublicClient>(
  customerChains.map((chain, index) => [
    chain.id,
    createPublicClient({ chain: customerViemChains[index], transport: http(chain.rpc) }) as PublicClient,
  ]),
);

export function customerChain(id: number): CustomerChain | undefined {
  return customerChains.find((chain) => chain.id === id);
}

export function explorerTx(chainId: number, hash: string): string {
  const base = chainId === shopChain.id ? shopChain.explorer : (customerChain(chainId)?.explorer ?? "");
  return `${base.replace(/\/$/, "")}/tx/${hash}`;
}

/** The wallet's USDT on the merchant chain, in base units. */
export async function usdtBalance(owner: Address): Promise<bigint> {
  return shopClient.readContract({ address: shopChain.usdt, abi: erc20Abi, functionName: "balanceOf", args: [owner] });
}

/** The wallet's USDC on a customer chain, in base units. */
export async function usdcBalance(chain: CustomerChain, owner: Address): Promise<bigint> {
  const client = customerClients.get(chain.id);
  if (!client) throw new Error(`no client for chain ${chain.id}`);
  return client.readContract({ address: chain.usdc, abi: erc20Abi, functionName: "balanceOf", args: [owner] });
}

/** The wallet's balance of any ERC-20 on a customer chain, in base units. */
export async function tokenBalance(chainId: number, token: Address, owner: Address): Promise<bigint> {
  const client = customerClients.get(chainId);
  if (!client) throw new Error(`no client for chain ${chainId}`);
  return client.readContract({ address: token, abi: erc20Abi, functionName: "balanceOf", args: [owner] });
}

/** The wallet's code on a customer chain: "0x" for a plain wallet, `0xef0100 ‖ delegate` after EIP-7702. */
export async function walletCode(chainId: number, owner: Address): Promise<Hex> {
  const client = customerClients.get(chainId);
  if (!client) throw new Error(`no client for chain ${chainId}`);
  return (await client.getCode({ address: owner })) ?? "0x";
}

/** The wallet's transaction nonce on a customer chain: what an EIP-7702 authorization must name. */
export async function walletNonce(chainId: number, owner: Address): Promise<number> {
  const client = customerClients.get(chainId);
  if (!client) throw new Error(`no client for chain ${chainId}`);
  return client.getTransactionCount({ address: owner, blockTag: "pending" });
}

/** A customer leg's token by address: USDC or a priced token. Unknown tokens read as 6-decimal "tokens". */
export function legToken(chainId: number, address: string): { symbol: string; decimals: number } {
  const chain = customerChain(chainId);
  const wanted = address.toLowerCase();
  if (!chain || chain.usdc.toLowerCase() === wanted) return { symbol: "USDC", decimals: chain?.usdcDecimals ?? 6 };
  const token = chain.tokens.find((candidate) => candidate.address.toLowerCase() === wanted);
  return token ? { symbol: token.symbol, decimals: token.decimals } : { symbol: "tokens", decimals: 6 };
}

/** USD prices by CoinGecko id, for estimates only: the Order Desk's quote sets the real amount. */
export async function usdPrices(ids: string[]): Promise<Record<string, number>> {
  const response = await fetch(
    `https://api.coingecko.com/api/v3/simple/price?ids=${ids.map(encodeURIComponent).join(",")}&vs_currencies=usd`,
    { cache: "no-store" },
  );
  if (!response.ok) throw new Error(`prices: http ${response.status}`);
  const body: Record<string, { usd?: number }> = await response.json();
  return Object.fromEntries(
    Object.entries(body)
      .filter(([, entry]) => typeof entry.usd === "number" && entry.usd > 0)
      .map(([id, entry]) => [id, entry.usd as number]),
  );
}

/**
 * About how much of a priced token pays `usd6` (USD in 6-decimal units, fee included): at `price`
 * USD per token, plus 1.5% for the quote's safety buffer, rounded up.
 */
export function estimateTokenUnits(usd6: bigint, price: number, decimals: number): bigint {
  const priceMicro = BigInt(Math.round(price * 1_000_000));
  if (priceMicro <= BigInt(0)) return BigInt(0);
  const numerator = usd6 * BigInt(10) ** BigInt(decimals) * BigInt(1015);
  const denominator = priceMicro * BigInt(1000);
  return (numerator + denominator - BigInt(1)) / denominator;
}

/** A token amount for people: exact up to 6 decimals (USDC), else 6 significant digits ("0.00341234" WETH). */
export function formatTokenAmount(units: bigint, decimals: number): string {
  if (decimals <= 6) return trimAmount(formatUnits(units, decimals));
  const [whole, fraction = ""] = formatUnits(units, decimals).split(".");
  if (whole !== "0") return trimAmount(`${whole}.${fraction.slice(0, Math.max(2, 6 - whole.length))}`);
  const lead = fraction.length - fraction.replace(/^0+/, "").length;
  return trimAmount(`0.${fraction.slice(0, lead + 6)}`);
}

/** What the customer locks for an order: the amount plus Bob's fee, rounded up (as the server). */
export function withProviderFee(units: bigint): bigint {
  const scaled = units * BigInt(10_000 + providerFeeBps);
  return (scaled + BigInt(9_999)) / BigInt(10_000);
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

/* ---------- The two locks of a cross-chain payment ---------- */

const LOCK_OPEN = 1;
const sourceLockAbi = parseAbi([
  "function locks(bytes32) view returns (address payer, uint64 expiry, uint8 state, address receiver, uint96 amount, bytes32 hashlock, bytes32 orderId, address token)",
]);
const destinationLockAbi = parseAbi([
  "function locks(bytes32) view returns (address maker, uint64 expiry, uint8 state, address merchant, uint96 amount, bytes32 hashlock, bytes32 orderId, bytes32 strategyHash, bytes32 srcLockId, uint64 srcChainId)",
]);

/** `DestinationLock.lockIdFor(srcChainId, srcLockId, maker)`: the one lock Bob can open for ours. */
export function destinationLockId(srcChainId: number, srcLockId: Hex, maker: Address): Hex {
  if (!shopChain.destinationLock) throw new Error("no DestinationLock in chains.json");
  return keccak256(
    encodeAbiParameters(
      [{ type: "bytes32" }, { type: "uint256" }, { type: "address" }, { type: "uint256" }, { type: "bytes32" }, { type: "address" }],
      [
        keccak256(toBytes("uniform.destinationlock.lock.v1")),
        BigInt(shopChain.id),
        shopChain.destinationLock,
        BigInt(srcChainId),
        srcLockId,
        maker,
      ],
    ),
  );
}

export type LockCheck = { ok: true } | { ok: false; reason: string };

/**
 * SPEC, before revealing the secret: read both locks on chain and check that Bob's lock pays this
 * merchant at least the order amount, under our hashlock, and ends well before the customer's lock.
 * The lock checked is the one only Bob can open for our source lock, recomputed here.
 */
export async function checkShopLock(args: {
  srcChainId: number;
  srcLockId: Hex;
  hashlock: Hex;
  merchant: Address;
  amountUnits: bigint;
}): Promise<LockCheck> {
  const chain = customerChain(args.srcChainId);
  const client = customerClients.get(args.srcChainId);
  if (!chain || !client || !shopChain.destinationLock) return { ok: false, reason: "unknown chain" };

  const [, srcExpiry, srcState, receiver, , srcHashlock] = await client.readContract({
    address: chain.sourceLock,
    abi: sourceLockAbi,
    functionName: "locks",
    args: [args.srcLockId],
  });
  if (srcState !== LOCK_OPEN || srcHashlock.toLowerCase() !== args.hashlock.toLowerCase()) {
    return { ok: false, reason: "your lock is not open under this payment's key" };
  }
  const id = destinationLockId(args.srcChainId, args.srcLockId, receiver);
  const [maker, expiry, state, merchant, amount, hashlock] = await shopClient.readContract({
    address: shopChain.destinationLock,
    abi: destinationLockAbi,
    functionName: "locks",
    args: [id],
  });
  const now = BigInt(Math.floor(Date.now() / 1000));
  if (state !== LOCK_OPEN) return { ok: false, reason: "the merchant's lock is not open yet" };
  if (maker.toLowerCase() !== receiver.toLowerCase()) return { ok: false, reason: "the merchant's lock is from someone else" };
  if (merchant.toLowerCase() !== args.merchant.toLowerCase()) return { ok: false, reason: "the lock pays another address" };
  if (amount < args.amountUnits) return { ok: false, reason: "the lock holds less than the order" };
  if (hashlock.toLowerCase() !== args.hashlock.toLowerCase()) return { ok: false, reason: "the lock is under another key" };
  if (expiry + BigInt(300) > srcExpiry) return { ok: false, reason: "the merchant's lock ends too close to yours" };
  if (expiry < now + BigInt(120)) return { ok: false, reason: "the merchant's lock is about to end" };
  return { ok: true };
}
