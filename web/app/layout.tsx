import type { Metadata } from "next";
import { Geist, Geist_Mono, Google_Sans } from "next/font/google";
import { Providers } from "./providers";
import "./globals.css";

const geist = Geist({ variable: "--font-geist", subsets: ["latin"] });
const geistMono = Geist_Mono({ variable: "--font-geist-mono", subsets: ["latin"] });
const display = Google_Sans({ variable: "--font-google-sans", subsets: ["latin"], weight: "400" });

export const metadata: Metadata = {
  title: "Uniform",
  description: "Stablecoin checkout. Shops receive USDT on Whitechain, customers pay no gas.",
};

// Inside the checkout widget's card (an iframe on a shop's page) the site's nav and side lines are
// hidden. Set before the first paint, so they never flash.
const MARK_EMBEDDED = "try{if(self!==top)document.documentElement.dataset.embedded=''}catch(e){document.documentElement.dataset.embedded=''}";

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html lang="en" className={`${geist.variable} ${geistMono.variable} ${display.variable}`} suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: MARK_EMBEDDED }} />
      </head>
      <body>
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
