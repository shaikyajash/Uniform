// The Demo Shop's products. Prices are in USDT and kept small: it runs on testnet money.
// The server prices the cart from this list; the browser's numbers are never trusted.

export type Product = { id: string; name: string; detail: string; price: string };

export const CATALOG: Product[] = [
  { id: "kettle", name: "Gooseneck kettle", detail: "0.9 L, brushed steel", price: "1.20" },
  { id: "cup", name: "Stoneware cup", detail: "Hand-thrown, 240 ml", price: "0.40" },
  { id: "filters", name: "Paper filters", detail: "Pack of 100", price: "0.15" },
];

export type Cart = Record<string, number>;

/** The cart's total in USDT base units (6 decimals), from the catalog's prices. */
export function totalUnits(cart: Cart): bigint {
  let total = BigInt(0);
  for (const product of CATALOG) {
    const count = Math.max(0, Math.min(9, Math.floor(cart[product.id] ?? 0)));
    const [whole, fraction = ""] = product.price.split(".");
    total += BigInt(count) * BigInt(whole + fraction.padEnd(6, "0"));
  }
  return total;
}

export function formatUnits6(units: bigint): string {
  const text = units.toString().padStart(7, "0");
  return `${text.slice(0, -6)}.${text.slice(-6, -4)}`;
}
