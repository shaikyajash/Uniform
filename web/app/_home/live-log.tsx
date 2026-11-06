"use client";

// One line that types out, holds, and moves on to the next: the "live" feel of a mock dashboard.

import { useEffect, useState } from "react";

const TYPE_MS = 28;
const HOLD_MS = 2_200;

export function LiveLog({ lines, className }: { lines: string[]; className?: string }) {
  const [index, setIndex] = useState(0);
  const [shown, setShown] = useState(0);

  useEffect(() => {
    const line = lines[index];
    const timer =
      shown < line.length
        ? setTimeout(() => setShown(shown + 1), TYPE_MS)
        : setTimeout(() => {
            setIndex((index + 1) % lines.length);
            setShown(0);
          }, HOLD_MS);
    return () => clearTimeout(timer);
  }, [index, shown, lines]);

  return (
    <p className={className} aria-live="off">
      <span aria-hidden="true">› </span>
      {lines[index].slice(0, shown)}
    </p>
  );
}
