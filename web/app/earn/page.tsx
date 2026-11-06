import type { Metadata } from "next";
import { Frame } from "@/app/_ui/frame";
import { Toaster } from "@/components/ui/sonner";
import { Earn } from "./earn";

export const metadata: Metadata = { title: "Earn · Uniform" };

/** `/earn`: a liquidity provider lends USDT to Uniform's payouts and sees what it earned. */
export default function EarnPage() {
  return (
    <main className="relative min-h-screen overflow-x-hidden px-[calc(var(--gutter)+20px)] pt-[max(120px,14vh)] pb-20">
      <Frame />
      <Earn />
      <Toaster position="bottom-right" />
    </main>
  );
}
