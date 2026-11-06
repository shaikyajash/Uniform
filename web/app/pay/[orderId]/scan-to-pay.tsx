"use client";

// "Scan to pay": the order's own pay page as a QR code, so a customer at a computer (or at a shop's
// screen) can pay from their phone instead. The card on this screen keeps following the order and
// shows "Payment confirmed" once the phone has paid.

import QRCode from "qrcode";
import { useEffect, useState, useSyncExternalStore } from "react";
import { FinePrint } from "./parts";

/** Only on screens with a mouse: a phone is already where the customer would pay. */
const POINTER = "(pointer: fine)";
function usePointerScreen(): boolean {
  return useSyncExternalStore(
    (changed) => {
      const query = window.matchMedia(POINTER);
      query.addEventListener("change", changed);
      return () => query.removeEventListener("change", changed);
    },
    () => window.matchMedia(POINTER).matches,
    () => false,
  );
}

export function ScanToPay({ orderId }: { orderId: string }) {
  const show = usePointerScreen();
  const [image, setImage] = useState<string | null>(null);

  useEffect(() => {
    if (!show) return;
    let current = true;
    // The full pay page, not the embedded one: the phone opens it on its own.
    const url = `${window.location.origin}/pay/${orderId}`;
    QRCode.toDataURL(url, { margin: 1, width: 360, color: { dark: "#000000", light: "#ffffff" } }).then(
      (data) => current && setImage(data),
      () => current && setImage(null),
    );
    return () => {
      current = false;
    };
  }, [show, orderId]);

  if (!show || !image) return null;
  return (
    <div className="grid justify-items-center gap-3 border-t border-hair pt-4">
      <span className="font-mono text-[10.5px] tracking-[0.08em] text-label">OR SCAN TO PAY ON YOUR PHONE</span>
      {/* eslint-disable-next-line @next/next/no-img-element -- a generated data URL */}
      <img src={image} alt="QR code of this order's pay page" width={160} height={160} className="rounded-md" />
      <FinePrint>This screen updates by itself once you&apos;ve paid.</FinePrint>
    </div>
  );
}
