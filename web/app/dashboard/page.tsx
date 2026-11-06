import type { Metadata } from "next";
import { Frame } from "@/app/_ui/frame";
import { Toaster } from "@/components/ui/sonner";
import { Dashboard } from "./dashboard";

export const metadata: Metadata = { title: "Dashboard · Uniform" };

/** `/dashboard`: the merchant's payments, refunds, webhooks and settings, behind a payout-wallet sign-in. */
export default function DashboardPage() {
  return (
    <main className="relative min-h-screen overflow-x-hidden px-[calc(var(--gutter)+20px)] pt-[max(120px,14vh)] pb-20">
      <Frame />
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-x-0 top-0 h-[480px] bg-[radial-gradient(50%_60%_at_50%_0%,rgba(255,255,255,0.05),transparent_70%)]"
      />
      <Dashboard />
      <Toaster position="bottom-right" />
    </main>
  );
}
