// The liquidity provider's side of Uniform, read and written straight on Whitechain.
//
// A provider (LP) lends USDT to Uniform through 1inch Aqua: they "ship" a strategy to the Payout
// Pool, which lets the pool take up to that much USDT from their wallet, only to pay a merchant
// for a customer payment that is already locked. The USDT stays in their wallet until then.
// "Docking" the strategy takes the permission back. Checked on a Whitechain fork (2026-11-07):
// ship, push (top-up of a live strategy, wallet unchanged) and dock all work as used below; a
// docked strategy cannot be shipped again, so a new one gets a new salt.
//
// History comes from the Whitechain explorer's API: Aqua's events have no indexed fields, so a
// node can't filter them by provider, and the explorer returns them decoded (and allows browsers).

import {
  createPublicClient,
  createWalletClient,
  custom,
  decodeAbiParameters,
  encodeAbiParameters,
  erc20Abi,
  http,
  keccak256,
  parseAbi,
  toBytes,
  type Address,
  type Hex,
} from "viem";
import type { ConnectedWallet } from "@privy-io/react-auth";
import { providerFeeBps, shopChain, shopViemChain } from "./chain";

/**
 * The Matchmaker's address: the only one allowed to fill from a strategy (`filler`). It is the
 * filler in every strategy Uniform's provider has shipped on Whitechain so far; it is not secret.
 */
export const UNIFORM_FILLER: Address =
  "0x70F748FFB7c53E853A5324df4263C242D1143d52";

/** The most one order may take from a new strategy: 100 USDT, the Relayer's default order cap. */
export const NEW_STRATEGY_MAX_PER_ORDER = BigInt(100_000_000);

const EXPLORER_API = `${shopChain.explorer.replace(/\/$/, "")}/api/v2`;

const client = createPublicClient({
  chain: shopViemChain,
  transport: http(shopChain.rpc),
});

const lockAbi = parseAbi([
  "function aqua() view returns (address)",
  "function pool() view returns (address)",
]);
const aquaAbi = parseAbi([
  "function rawBalances(address maker, address app, bytes32 strategyHash, address token) view returns (uint248 balance, uint8 tokensCount)",
  "function ship(address app, bytes strategy, address[] tokens, uint256[] amounts) returns (bytes32)",
  "function push(address maker, address app, bytes32 strategyHash, address token, uint256 amount)",
  "function dock(address app, bytes32 strategyHash, address[] tokens)",
]);

const STRATEGY = [
  {
    type: "tuple",
    components: [
      { name: "maker", type: "address" },
      { name: "token", type: "address" },
      { name: "filler", type: "address" },
      { name: "maxPerOrder", type: "uint128" },
      { name: "feeBps", type: "uint16" },
      { name: "salt", type: "bytes32" },
    ],
  },
] as const;

export type Strategy = {
  maker: Address;
  token: Address;
  filler: Address;
  maxPerOrder: bigint;
  feeBps: number;
  salt: Hex;
};

/** One strategy the provider shipped, with what Aqua says about it now. */
export type Shipped = {
  hash: Hex;
  strategy: Strategy;
  /** USDT the pool may still take; 0 once docked. */
  balance: bigint;
  /** Docked: closed for good. */
  docked: boolean;
  /** Uniform's Matchmaker can fill from it (it is the filler and the token is USDT). */
  forUniform: boolean;
  shippedTx: Hex;
};

export type FillRecord = {
  orderId: Hex;
  amount: bigint;
  lockId: Hex;
  tx: Hex;
  block: number;
};

export type Position = {
  aqua: Address;
  pool: Address;
  walletUsdt: bigint;
  /** WBT in the wallet, which pays gas for lending, adding and withdrawing. */
  walletGas: bigint;
  /** What the wallet lets Aqua move. */
  allowance: bigint;
  strategies: Shipped[];
  fills: FillRecord[];
};

/** Aqua and the Payout Pool, read off the DestinationLock the app already knows. */
async function contracts(): Promise<{ aqua: Address; pool: Address }> {
  if (!shopChain.destinationLock)
    throw new Error("no DestinationLock in chains.json");
  const [aqua, pool] = await Promise.all([
    client.readContract({
      address: shopChain.destinationLock,
      abi: lockAbi,
      functionName: "aqua",
    }),
    client.readContract({
      address: shopChain.destinationLock,
      abi: lockAbi,
      functionName: "pool",
    }),
  ]);
  return { aqua, pool };
}

type ExplorerLog = {
  transaction_hash: Hex;
  block_number: number;
  decoded: {
    method_call: string;
    parameters: { name: string; value: string }[];
  } | null;
};

/** Every decoded log of `address`, newest first (a few pages at most: a testnet holds few). */
async function explorerLogs(address: Address): Promise<ExplorerLog[]> {
  const logs: ExplorerLog[] = [];
  let query = "";
  for (let page = 0; page < 10; page++) {
    const response = await fetch(
      `${EXPLORER_API}/addresses/${address}/logs${query}`,
    );
    if (!response.ok)
      throw new Error(`the Whitechain explorer answered ${response.status}`);
    const body = (await response.json()) as {
      items: ExplorerLog[];
      next_page_params: Record<string, string> | null;
    };
    logs.push(...body.items);
    if (!body.next_page_params) break;
    query = `?${new URLSearchParams(body.next_page_params).toString()}`;
  }
  return logs;
}

function params(log: ExplorerLog): Record<string, string> {
  return Object.fromEntries(
    (log.decoded?.parameters ?? []).map((p) => [p.name, p.value]),
  );
}

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

export async function readPosition(maker: Address): Promise<Position> {
  const { aqua, pool } = await contracts();
  const [walletUsdt, walletGas, allowance, aquaLogs, poolLogs] =
    await Promise.all([
      client.readContract({
        address: shopChain.usdt,
        abi: erc20Abi,
        functionName: "balanceOf",
        args: [maker],
      }),
      client.getBalance({ address: maker }),
      client.readContract({
        address: shopChain.usdt,
        abi: erc20Abi,
        functionName: "allowance",
        args: [maker, aqua],
      }),
      explorerLogs(aqua),
      explorerLogs(pool),
    ]);

  const shippedLogs = aquaLogs.filter((log) => {
    if (!log.decoded?.method_call.startsWith("Shipped(")) return false;
    const p = params(log);
    return same(p.maker, maker) && same(p.app, pool);
  });
  const strategies = await Promise.all(
    shippedLogs.map(async (log): Promise<Shipped> => {
      const p = params(log);
      const hash = p.strategyHash as Hex;
      const [decoded] = decodeAbiParameters(STRATEGY, p.strategy as Hex);
      const [balance, tokensCount] = await client.readContract({
        address: aqua,
        abi: aquaAbi,
        functionName: "rawBalances",
        args: [maker, pool, hash, shopChain.usdt],
      });
      const docked = tokensCount === 255;
      return {
        hash,
        strategy: {
          ...decoded,
          maxPerOrder: BigInt(decoded.maxPerOrder),
          feeBps: Number(decoded.feeBps),
        },
        balance: docked ? BigInt(0) : BigInt(balance),
        docked,
        forUniform:
          same(decoded.filler, UNIFORM_FILLER) &&
          same(decoded.token, shopChain.usdt),
        shippedTx: log.transaction_hash,
      };
    }),
  );

  // The pool's own record of every payout it made from this provider's strategies.
  const fills = poolLogs
    .filter(
      (log) =>
        log.decoded?.method_call.startsWith("Filled(") &&
        same(params(log).maker, maker),
    )
    .map((log): FillRecord => {
      const p = params(log);
      return {
        orderId: p.orderId as Hex,
        amount: BigInt(p.amount),
        lockId: p.lockId as Hex,
        tx: log.transaction_hash,
        block: log.block_number,
      };
    });

  return { aqua, pool, walletUsdt, walletGas, allowance, strategies, fills };
}

/** What the pool can actually take now: the strategy's limit, capped by the wallet and its approval. */
export function payableNow(position: Position): bigint {
  const shipped = position.strategies
    .filter((s) => !s.docked && s.forUniform)
    .reduce((sum, s) => sum + s.balance, BigInt(0));
  const min = (a: bigint, b: bigint) => (a < b ? a : b);
  return min(shipped, min(position.walletUsdt, position.allowance));
}

/**
 * Fees earned, estimated: the provider's fee rate times what they paid out. The real fee arrives
 * as USDC on the customer's chain when the provider claims, which this page doesn't read.
 */
export function estimatedFees(position: Position): bigint {
  const volume = position.fills.reduce(
    (sum, fill) => sum + fill.amount,
    BigInt(0),
  );
  return (volume * BigInt(providerFeeBps)) / BigInt(10_000);
}

/** Sends one transaction from the provider's wallet on Whitechain and waits for it. */
async function send(
  wallet: ConnectedWallet,
  call: {
    address: Address;
    abi: typeof aquaAbi | typeof erc20Abi;
    functionName: string;
    args: readonly unknown[];
  },
): Promise<Hex> {
  await wallet.switchChain(shopChain.id);
  const provider = await wallet.getEthereumProvider();
  const walletClient = createWalletClient({
    account: wallet.address as Address,
    chain: shopViemChain,
    transport: custom(provider),
  });
  // viem's typed overloads don't narrow over a union of ABIs; the calls below are typed by hand.
  const hash = await walletClient.writeContract(
    call as Parameters<typeof walletClient.writeContract>[0],
  );
  const receipt = await client.waitForTransactionReceipt({ hash });
  if (receipt.status !== "success")
    throw new Error("the transaction failed on Whitechain");
  return hash;
}

/** Says which wallet window is open, so a second one doesn't look like a repeat. */
export type OnStep = (step: string) => void;

/**
 * Lets Aqua move `needed` USDT from the wallet, if it can't already. Reports the steps:
 * "1 of 2" for the approval when one is needed, then `final` for the call that follows.
 */
async function approveIfNeeded(
  wallet: ConnectedWallet,
  aqua: Address,
  current: bigint,
  needed: bigint,
  final: string,
  onStep: OnStep,
) {
  if (current >= needed) {
    onStep(final);
    return;
  }
  onStep("Step 1 of 2: allow Aqua to use your USDT");
  await send(wallet, {
    address: shopChain.usdt,
    abi: erc20Abi,
    functionName: "approve",
    args: [aqua, needed],
  });
  onStep(`Step 2 of 2: ${final.charAt(0).toLowerCase()}${final.slice(1)}`);
}

/** A new strategy for Uniform: Uniform's filler, USDT, the advertised fee, a fresh salt. */
export async function startProviding(
  wallet: ConnectedWallet,
  position: Position,
  amount: bigint,
  onStep: OnStep = () => {},
): Promise<Hex> {
  const maker = wallet.address as Address;
  const strategy = {
    maker,
    token: shopChain.usdt,
    filler: UNIFORM_FILLER,
    maxPerOrder: NEW_STRATEGY_MAX_PER_ORDER,
    feeBps: providerFeeBps,
    salt: keccak256(toBytes(`uniform-earn:${maker}:${Date.now()}`)),
  };
  const encoded = encodeAbiParameters(STRATEGY, [strategy]);
  await approveIfNeeded(
    wallet,
    position.aqua,
    position.allowance,
    payableTotal(position) + amount,
    "Lend it to Uniform",
    onStep,
  );
  return send(wallet, {
    address: position.aqua,
    abi: aquaAbi,
    functionName: "ship",
    args: [position.pool, encoded, [shopChain.usdt], [amount]],
  });
}

/** Raises a live strategy's limit. The wallet keeps its USDT; only the limit grows. */
export async function addLiquidity(
  wallet: ConnectedWallet,
  position: Position,
  shipped: Shipped,
  amount: bigint,
  onStep: OnStep = () => {},
): Promise<Hex> {
  await approveIfNeeded(
    wallet,
    position.aqua,
    position.allowance,
    payableTotal(position) + amount,
    "Raise your limit",
    onStep,
  );
  return send(wallet, {
    address: position.aqua,
    abi: aquaAbi,
    functionName: "push",
    args: [
      wallet.address as Address,
      position.pool,
      shipped.hash,
      shopChain.usdt,
      amount,
    ],
  });
}

/** Closes a strategy for good: the pool can take nothing more from it. */
export async function withdraw(
  wallet: ConnectedWallet,
  position: Position,
  shipped: Shipped,
): Promise<Hex> {
  return send(wallet, {
    address: position.aqua,
    abi: aquaAbi,
    functionName: "dock",
    args: [position.pool, shipped.hash, [shopChain.usdt]],
  });
}

/** Everything the live strategies may take, which the wallet's approval has to cover. */
function payableTotal(position: Position): bigint {
  return position.strategies
    .filter((s) => !s.docked)
    .reduce((sum, s) => sum + s.balance, BigInt(0));
}

/**
 * WBT enough for an approve and a ship (two transactions). One cost about 0.000325 WBT on
 * Whitechain testnet (from a failed attempt's "want" figure), so 0.001 leaves room.
 */
export const MIN_GAS_WBT = BigInt(1_000_000_000_000_000);

/** Free test WBT: GitHub sign-in, 0.5 WBT a day. */
export const WBT_FAUCET = "https://faucet.testnet.whitechain.io";
