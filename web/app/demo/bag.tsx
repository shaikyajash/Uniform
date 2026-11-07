// A coffee bag, drawn: a sealed top, the one-way valve, and a paper label with the coffee's name.
// Every product shares it; only the colour and the label change.

import { useId } from "react";
import type { Product } from "./catalog";

export function Bag({ product, className }: { product: Product; className?: string }) {
  const id = useId().replace(/:/g, "");
  const shade = `${id}-shade`;
  const lines = product.label;
  return (
    <svg className={className} viewBox="0 0 200 270" role="img" aria-label={`${product.name} coffee bag`}>
      <defs>
        {/* Light from the left: a soft highlight, then the far side falls into shadow. */}
        <linearGradient id={shade} x1="0" x2="1">
          <stop offset="0" stopColor="#fff" stopOpacity="0.16" />
          <stop offset="0.22" stopColor="#fff" stopOpacity="0.04" />
          <stop offset="0.7" stopColor="#000" stopOpacity="0" />
          <stop offset="1" stopColor="#000" stopOpacity="0.22" />
        </linearGradient>
      </defs>

      <ellipse cx="100" cy="258" rx="78" ry="7" fill="#000" opacity="0.14" />

      {/* Body, a little wider at the base where the coffee settles. */}
      <path d="M32 36 H168 L176 244 Q176 252 168 252 H32 Q24 252 24 244 Z" fill={product.color} />
      <path d="M32 36 H168 L176 244 Q176 252 168 252 H32 Q24 252 24 244 Z" fill={`url(#${shade})`} />
      {/* The side folds. */}
      <path d="M40 40 L36 248 M160 40 L164 248" stroke="#000" strokeOpacity="0.1" strokeWidth="1.2" />

      {/* Heat-sealed top. */}
      <rect x="30" y="12" width="140" height="28" rx="2" fill={product.color} />
      <rect x="30" y="12" width="140" height="28" rx="2" fill="#000" opacity="0.14" />
      {Array.from({ length: 34 }, (_, i) => (
        <path key={i} d={`M${34 + i * 4} 16 V36`} stroke="#000" strokeOpacity="0.12" strokeWidth="1" />
      ))}

      {/* The one-way valve. */}
      <circle cx="100" cy="64" r="7" fill="#000" opacity="0.12" />
      <circle cx="100" cy="64" r="3" fill="#000" opacity="0.18" />

      {/* Label. */}
      <rect x="46" y="92" width="108" height="128" rx="2" fill="#f7f2e8" />
      <text x="100" y="114" textAnchor="middle" fontSize="7.5" letterSpacing="2.6" fill="#1b1a17" style={{ fontFamily: "var(--font-geist), system-ui, sans-serif" }}>
        MONSOON
      </text>
      <path d="M58 122 H142" stroke="#1b1a17" strokeOpacity="0.25" strokeWidth="0.8" />
      {lines.map((line, i) => (
        <text
          key={line}
          x="100"
          y={150 + i * 20 - (lines.length - 1) * 10}
          textAnchor="middle"
          fontSize="19"
          fill={product.color}
          style={{ fontFamily: "var(--font-merchant), Georgia, serif" }}
        >
          {line}
        </text>
      ))}
      <text x="100" y="190" textAnchor="middle" fontSize="6.5" letterSpacing="1.4" fill="#1b1a17" fillOpacity="0.6" style={{ fontFamily: "var(--font-geist), system-ui, sans-serif" }}>
        {product.region.toUpperCase()}
      </text>
      <text x="100" y="208" textAnchor="middle" fontSize="6.5" letterSpacing="1.4" fill="#1b1a17" style={{ fontFamily: "var(--font-geist), system-ui, sans-serif" }}>
        {`${product.roast.toUpperCase()} ROAST · 250 G`}
      </text>
    </svg>
  );
}
