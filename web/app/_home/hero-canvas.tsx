"use client";

// The hero's background: the light ray (see ray-shader.ts), with a fade at the bottom for the
// logo strip.

import { useEffect, useRef } from "react";
import { startRay } from "./ray-shader";
import styles from "./home.module.css";

export function HeroCanvas() {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const stage = ref.current;
    if (!stage) return;
    // A new canvas each time: stopping the ray ends its WebGL context, and a canvas whose
    // context has ended cannot draw again (React runs this effect twice in development).
    const canvas = document.createElement("canvas");
    canvas.className = styles.heroCanvas;
    stage.prepend(canvas);
    const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    let stop: (() => void) | undefined;
    try {
      stop = startRay(canvas, still);
    } catch (error) {
      // No WebGL: the hero still works on plain black.
      console.warn("The hero light could not start:", error);
    }
    return () => {
      stop?.();
      canvas.remove();
    };
  }, []);

  return (
    <div ref={ref} className={styles.heroStage} aria-hidden="true">
      <div className={styles.heroShadeDown} />
    </div>
  );
}
