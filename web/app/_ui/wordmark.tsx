// The Uniform wordmark, drawn rather than typeset.
//
// Every letter is one line of the same width (STROKE) on the same 20-unit cap height: a uniform
// mark for Uniform. Joins are sharp and the ends are cut flat at the cap line and the baseline
// (the clip), as a typeface does. Round and open letters (O, the open side of F) get a little
// less space so the gaps look even. The colour is the text colour (`currentColor`).

import { useId } from "react";

const STROKE = 2.2;
const HEIGHT = 20;
const WIDTH = 130.4;

/** Each letter's left edge and its path (drawn from its own origin). */
const LETTERS: { x: number; d: string }[] = [
  // U: two stems into a half circle.
  { x: 0, d: "M1.1 0V12.7a6.4 6.4 0 0 0 12.8 0V0" },
  // N
  { x: 21.5, d: "M1.1 20V0L13.9 20V0" },
  // I
  { x: 43, d: "M1.1 0V20" },
  // F: the middle bar a little shorter than the top one.
  { x: 51.9, d: "M1.1 20V1.1H11.5M1.1 10.4H9.6" },
  // O
  { x: 68.9, d: "M10 1.1a8.9 8.9 0 1 1 0 17.8a8.9 8.9 0 1 1 0-17.8Z" },
  // R: a bowl and a straight leg.
  { x: 94.4, d: "M1.1 20V1.1H7a4.95 4.95 0 0 1 0 9.9H1.1M6.4 11L12.8 20" },
  // M: the middle point stops above the baseline.
  { x: 114.2, d: "M1.1 20V0L8 13.2L14.9 0V20" },
];

export function Wordmark({ className }: { className?: string }) {
  const clip = `${useId().replace(/:/g, "")}-clip`;
  return (
    <svg className={className} viewBox={`0 0 ${WIDTH} ${HEIGHT}`} role="img" aria-label="Uniform">
      <clipPath id={clip}>
        <rect width={WIDTH} height={HEIGHT} />
      </clipPath>
      <g
        clipPath={`url(#${clip})`}
        fill="none"
        stroke="currentColor"
        strokeWidth={STROKE}
        strokeLinejoin="miter"
        strokeMiterlimit={10}
      >
        {LETTERS.map((letter) => (
          <path key={letter.x} d={letter.d} transform={`translate(${letter.x} 0)`} />
        ))}
      </g>
    </svg>
  );
}
