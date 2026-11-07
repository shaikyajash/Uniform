// The demo store's products. Prices are in USDT and kept small: it runs on testnet money.
// The server prices the cart from this list; the browser's numbers are never trusted.

export type Roast = "Light" | "Medium" | "Dark";

export type Product = {
  id: string;
  name: string;
  /** The name as it sits on the bag's label, one entry per line. */
  label: string[];
  region: string;
  notes: string[];
  roast: Roast;
  price: string;
  /** The bag's colour, and the lighter colour of the card behind it. */
  color: string;
  tint: string;
};

export const STORE = { name: "Monsoon", full: "Monsoon Coffee Roasters", referencePrefix: "MSN" };

export const CATALOG: Product[] = [
  {
    id: "araku",
    name: "Araku Valley",
    label: ["Araku", "Valley"],
    region: "Andhra Pradesh",
    notes: ["Jaggery", "Orange", "Black tea"],
    roast: "Light",
    price: "2",
    color: "#d9822b",
    tint: "#f1dcc0",
  },
  {
    id: "chikmagalur",
    name: "Chikmagalur",
    label: ["Chikma-", "galur"],
    region: "Karnataka",
    notes: ["Milk chocolate", "Almond", "Fig"],
    roast: "Medium",
    price: "1",
    color: "#a8432f",
    tint: "#ecd2c6",
  },
  {
    id: "coorg",
    name: "Coorg Peaberry",
    label: ["Coorg", "Peaberry"],
    region: "Karnataka",
    notes: ["Cherry", "Cacao nib", "Honey"],
    roast: "Medium",
    price: "2",
    color: "#5d6b9a",
    tint: "#d8dbe8",
  },
  {
    id: "malabar",
    name: "Monsooned Malabar",
    label: ["Monsooned", "Malabar"],
    region: "Malabar Coast",
    notes: ["Cedar", "Cocoa", "Spice"],
    roast: "Dark",
    price: "1",
    color: "#1f4b45",
    tint: "#cfdcd6",
  },
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
  const cents = text.slice(-6, -4);
  return cents === "00" ? text.slice(0, -6) : `${text.slice(0, -6)}.${cents}`;
}
