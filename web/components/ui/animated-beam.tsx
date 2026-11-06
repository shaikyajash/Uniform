"use client";

import { useEffect, useId, useRef, useState, type RefObject } from "react";
import { animate, motion, useMotionValue, useTransform, type AnimationPlaybackControls, type MotionValue } from "motion/react";

type BeamProps = {
  containerRef: RefObject<HTMLElement | null>;
  fromRef: RefObject<HTMLElement | null>;
  toRef: RefObject<HTMLElement | null>;
  /** How far the line bends. Positive bends up, as in the stock beam. */
  curvature?: number;
  endYOffset?: number;
  startYOffset?: number;
  /** Run the light from `toRef` back to `fromRef`. */
  reverse?: boolean;
  /** Freeze the light in place. */
  paused?: boolean;
  /** Brighten this path slowly, e.g. while a node it links to is hovered. */
  highlighted?: boolean;
  /** Seconds for one pass. */
  duration?: number;
  delay?: number;
  pathColor?: string;
  pathOpacity?: number;
  pathWidth?: number;
  gradientStartColor?: string;
  gradientStopColor?: string;
  /** Image that rides the line from start to end, e.g. a token logo. */
  deliver?: string;
};

type Geometry = { width: number; height: number; sx: number; sy: number; ex: number; ey: number };

function point(t: number, g: Geometry, cy: number) {
  const cx = (g.sx + g.ex) / 2;
  const u = 1 - t;
  return {
    x: u * u * g.sx + 2 * u * t * cx + t * t * g.ex,
    y: u * u * g.sy + 2 * u * t * cy + t * t * g.ey,
  };
}

/** The stock beam: a gradient of light runs along a curved line, and it can be paused. */
export function AnimatedBeam({
  containerRef,
  fromRef,
  toRef,
  curvature = 0,
  endYOffset = 0,
  startYOffset = 0,
  reverse = false,
  paused = false,
  highlighted = false,
  duration = 4,
  delay = 0,
  pathColor = "#ffffff",
  pathOpacity = 0.12,
  pathWidth = 1.5,
  gradientStartColor = "#6f9bff",
  gradientStopColor = "#dbe7ff",
  deliver,
}: BeamProps) {
  const uid = useId().replace(/:/g, "");
  const gradientId = `beam-grad-${uid}`;
  const pathId = `beam-path-${uid}`;
  const [geo, setGeo] = useState<Geometry | null>(null);
  const geoRef = useRef<Geometry | null>(null);
  const pausedRef = useRef(paused);
  const progress: MotionValue<number> = useMotionValue(0);
  const controls = useRef<AnimationPlaybackControls | null>(null);
  pausedRef.current = paused;

  // A normal effect: the container's ref is attached after its children's layout effects.
  useEffect(() => {
    const container = containerRef.current;
    const from = fromRef.current;
    const to = toRef.current;
    if (!container || !from || !to) return;

    const measure = () => {
      const box = container.getBoundingClientRect();
      const a = from.getBoundingClientRect();
      const b = to.getBoundingClientRect();
      const next: Geometry = {
        width: box.width,
        height: box.height,
        sx: a.left - box.left + a.width / 2,
        sy: a.top - box.top + a.height / 2 + startYOffset,
        ex: b.left - box.left + b.width / 2,
        ey: b.top - box.top + b.height / 2 + endYOffset,
      };
      geoRef.current = next;
      setGeo(next);
    };

    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(container);
    observer.observe(from);
    observer.observe(to);
    return () => observer.disconnect();
  }, [containerRef, fromRef, toRef, startYOffset, endYOffset]);

  const ready = geo !== null;

  // One shared clock for the gradient and the token. Its controls let hover pause it.
  useEffect(() => {
    if (!ready) return;
    controls.current = animate(progress, 1, { duration, delay, repeat: Infinity, ease: "linear" });
    if (pausedRef.current) controls.current.pause();
    return () => controls.current?.stop();
  }, [ready, duration, delay, progress]);

  useEffect(() => {
    if (paused) controls.current?.pause();
    else controls.current?.play();
  }, [paused, ready]);

  // The light: a bright band that sweeps along the line, direction flipped when reversed.
  const gx1 = useTransform(progress, [0, 1], reverse ? ["90%", "-10%"] : ["10%", "110%"]);
  const gx2 = useTransform(progress, [0, 1], reverse ? ["100%", "0%"] : ["0%", "100%"]);

  // The token's position along the curve, read from the same clock.
  const at = (t: number) => {
    const g = geoRef.current;
    if (!g) return { x: 0, y: 0 };
    return point(reverse ? 1 - t : t, g, g.sy - curvature);
  };
  const x = useTransform(progress, (t) => at(t).x);
  const y = useTransform(progress, (t) => at(t).y);

  if (!geo) return null;

  const { width, height, sx, sy, ex, ey } = geo;
  const cy = sy - curvature;
  const d = `M ${sx} ${sy} Q ${(sx + ex) / 2} ${cy} ${ex} ${ey}`;

  return (
    <svg
      aria-hidden="true"
      width={width}
      height={height}
      style={{ position: "absolute", inset: 0, overflow: "visible", pointerEvents: "none" }}
    >
      <defs>
        <motion.linearGradient id={gradientId} gradientUnits="userSpaceOnUse" x1={gx1} x2={gx2} y1="0%" y2="0%">
          <stop stopColor={gradientStartColor} stopOpacity="0" />
          <stop stopColor={gradientStartColor} />
          <stop offset="32.5%" stopColor={gradientStopColor} />
          <stop offset="100%" stopColor={gradientStopColor} stopOpacity="0" />
        </motion.linearGradient>
      </defs>
      <motion.path
        id={pathId}
        d={d}
        stroke={pathColor}
        strokeLinecap="round"
        fill="none"
        initial={false}
        animate={{ strokeOpacity: highlighted ? 0.55 : pathOpacity, strokeWidth: highlighted ? pathWidth * 1.8 : pathWidth }}
        transition={{ duration: 1.1, ease: [0.22, 1, 0.36, 1] }}
      />
      <path d={d} stroke={`url(#${gradientId})`} strokeWidth={pathWidth} strokeLinecap="round" fill="none" />
      {deliver && (
        <motion.g style={{ x, y }}>
          <circle r={12} fill="#050506" stroke="rgba(255,255,255,0.2)" />
          <image href={deliver} x={-9} y={-9} width={18} height={18} />
        </motion.g>
      )}
    </svg>
  );
}
