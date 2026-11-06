// The vortex's shape: where a point sits for a given place along a strand.
//
// A strand runs from s = 0 (out on the floor) to s = 1 (the top of the funnel). Three smooth
// curves of s give its distance from the centre, its height and how far it has turned. Each
// strand is the same path, started at a different angle ("lane").

const TAU = Math.PI * 2;
const TABLE_SIZE = 1024;

/** Sizes in scene units (the original's pixel settings divided by 60). */
const FLOOR_RADIUS = 1150 / 60;
const WAIST_RADIUS = 53 / 60;
const CROWN_RADIUS = 380 / 60;
/** Where along a strand the narrowest point is. */
const WAIST_AT = 0.5;
/** Whole turns a strand makes from the floor to the top. */
const TWIST = 3;

type Knot = [x: number, y: number];

/** A smooth curve through the knots that never overshoots them (monotone cubic Hermite). */
function smoothThrough(knots: Knot[]): (x: number) => number {
  const n = knots.length;
  const slope = knots.slice(0, -1).map(([x, y], i) => (knots[i + 1][1] - y) / (knots[i + 1][0] - x));
  const tangent = knots.map((_, i) => {
    if (i === 0) return slope[0];
    if (i === n - 1) return slope[n - 2];
    return slope[i - 1] * slope[i] <= 0 ? 0 : (slope[i - 1] + slope[i]) / 2;
  });
  for (let i = 0; i < n - 1; i++) {
    if (Math.abs(slope[i]) < 1e-12) {
      tangent[i] = tangent[i + 1] = 0;
      continue;
    }
    const a = tangent[i] / slope[i];
    const b = tangent[i + 1] / slope[i];
    const length = a * a + b * b;
    if (length > 9) {
      const scale = 3 / Math.sqrt(length);
      tangent[i] = scale * a * slope[i];
      tangent[i + 1] = scale * b * slope[i];
    }
  }
  return (x) => {
    if (x <= knots[0][0]) return knots[0][1];
    if (x >= knots[n - 1][0]) return knots[n - 1][1];
    let i = 0;
    while (i < n - 2 && knots[i + 1][0] < x) i++;
    const width = knots[i + 1][0] - knots[i][0];
    const t = (x - knots[i][0]) / width;
    const t2 = t * t;
    const t3 = t2 * t;
    return (
      (2 * t3 - 3 * t2 + 1) * knots[i][1] +
      (t3 - 2 * t2 + t) * width * tangent[i] +
      (-2 * t3 + 3 * t2) * knots[i + 1][1] +
      (t3 - t2) * width * tangent[i + 1]
    );
  };
}

/** The curve sampled once, so each frame reads a table instead of solving the curve. */
function tabulate(curve: (x: number) => number): Float32Array {
  const table = new Float32Array(TABLE_SIZE);
  for (let i = 0; i < TABLE_SIZE; i++) table[i] = curve(i / (TABLE_SIZE - 1));
  return table;
}

function read(table: Float32Array, s: number): number {
  if (s <= 0) return table[0];
  const last = table.length - 1;
  if (s >= 1) return table[last];
  const at = s * last;
  const i = at | 0;
  return table[i] + (table[i + 1] - table[i]) * (at - i);
}

const w = WAIST_AT;
const RADIUS = tabulate(
  smoothThrough([
    [0, FLOOR_RADIUS],
    [0.24 * w, 0.667 * FLOOR_RADIUS],
    [0.5 * w, 0.3 * FLOOR_RADIUS],
    [0.76 * w, 0.08 * FLOOR_RADIUS],
    [w, WAIST_RADIUS],
    [w + 0.3 * (1 - w), 0.2 * CROWN_RADIUS],
    [w + 0.6 * (1 - w), 0.44 * CROWN_RADIUS],
    [1, CROWN_RADIUS],
  ]),
);
const HEIGHT = tabulate(
  smoothThrough([
    [0, 0],
    [0.1, 0.2],
    [0.2, 0.8],
    [0.35, 2],
    [0.5, 3.8],
    [0.75, 7],
    [1, 10],
  ]),
);
const turns = TWIST * TAU;
const ANGLE = tabulate(
  smoothThrough([
    [0, 0],
    [0.15, 0.15 * turns],
    [0.25, 0.25 * turns],
    [0.45, 0.55 * turns],
    [0.6, 0.7 * turns],
    [0.8, 0.88 * turns],
    [1, turns],
  ]),
);

/** How far strands wobble in and out, as a share of their radius. */
const WOBBLE = 0.008;

/**
 * Writes the point at place `s` on the strand in `lane` into `out` at `index` (x, y, z).
 * `spin` is how far the whole vortex has turned; `wobblePhase` and `time` move the small wobble.
 */
export function writePoint(
  out: Float32Array,
  index: number,
  s: number,
  lane: number,
  spin: number,
  wobblePhase: number,
  time: number,
): void {
  const radius = read(RADIUS, s);
  const angle = read(ANGLE, s) + lane + spin;
  const distance = radius + Math.sin(25 * s + wobblePhase + 0.3 * time) * WOBBLE * radius;
  out[index] = Math.cos(angle) * distance;
  out[index + 1] = read(HEIGHT, s);
  out[index + 2] = Math.sin(angle) * distance;
}

export function laneOf(strand: number, strands: number): number {
  return (strand / strands) * TAU;
}

export { TAU };
