"use client";

// The hero's background: the 3D vortex (see vortex-scene.ts) with two dark gradients over it so
// the headline stays readable.

import { useEffect, useRef } from "react";
import { startVortex } from "./vortex-scene";
import styles from "./home.module.css";

export function HeroCanvas() {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const stage = ref.current;
    if (!stage) return;
    // A new canvas each time: stopping the vortex ends its WebGL context, and a canvas whose
    // context has ended cannot draw again (React runs this effect twice in development).
    const canvas = document.createElement("canvas");
    canvas.className = styles.heroCanvas;
    stage.prepend(canvas);
    const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    let stop: (() => void) | undefined;
    try {
      stop = startVortex(canvas, still);
    } catch (error) {
      // No WebGL: the hero still works on plain black.
      console.warn("Vortex could not start:", error);
    }
    return () => {
      stop?.();
      canvas.remove();
    };
  }, []);

  return (
    <div ref={ref} className={styles.heroStage} aria-hidden="true">
      <div className={styles.heroShadeSide} />
      <div className={styles.heroShadeDown} />
    </div>
  );
}
