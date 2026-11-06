import type { Metadata } from "next";
import { Instrument_Serif } from "next/font/google";
import Script from "next/script";
import { Shop } from "./shop";

const serif = Instrument_Serif({ subsets: ["latin"], weight: "400", variable: "--font-shop" });

export const metadata: Metadata = {
  title: "Kettle & Co. — a Uniform demo shop",
  description: "A small shop that takes stablecoin payments with Uniform's checkout widget.",
};

/** `/demo`: a stand-in for a shop's own site. It adds Uniform with one script and one call. */
export default function DemoPage() {
  return (
    <div className={`${serif.variable} min-h-screen bg-[#f3eee5] text-[#1f1b16]`}>
      <Script src="/widget.js" strategy="afterInteractive" />
      <Shop />
    </div>
  );
}
