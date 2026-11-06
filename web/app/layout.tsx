import type { Metadata } from "next";
import { Inter, Outfit } from "next/font/google";
import { Providers } from "./providers";
import "./globals.css";

const inter = Inter({ variable: "--font-inter", subsets: ["latin"] });
const outfit = Outfit({ variable: "--font-outfit", subsets: ["latin"], weight: ["500", "600"] });

export const metadata: Metadata = {
  title: "Uniform",
  description: "Pay from any chain. Shops receive USDT on Whitechain.",
};

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html lang="en" className={`${inter.variable} ${outfit.variable} !bg-black`}>
      <body className="bg-black">
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
