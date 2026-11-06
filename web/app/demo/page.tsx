import type { Metadata } from "next";
import { Instrument_Serif } from "next/font/google";
import Script from "next/script";
import { Store } from "./shop";

const serif = Instrument_Serif({ subsets: ["latin"], weight: "400", variable: "--font-merchant" });

export const metadata: Metadata = {
  title: "Monsoon Coffee Roasters",
  description: "Single-estate Indian coffee, roasted in Bombay. A demo store that takes payments with Uniform.",
};

/** `/demo`: a stand-in for a merchant's own site. It adds Uniform with one script and one call. */
export default function DemoPage() {
  return (
    <div className={serif.variable}>
      <Script src="/widget.js" strategy="afterInteractive" />
      <Store />
    </div>
  );
}
