"use client";

// "Add Uniform to your site": the two pieces a merchant pastes, with this site's own address.

import { Panel } from "./bits";
import { CopyButton } from "./settings";

export function Integrate() {
  const origin = typeof window === "undefined" ? "" : window.location.origin;
  const server = `// On your server. The API key stays here, never in the browser.
const response = await fetch("${origin}/desk/v1/orders", {
  method: "POST",
  headers: {
    "content-type": "application/json",
    authorization: \`Bearer \${process.env.UNIFORM_API_KEY}\`,
  },
  body: JSON.stringify({ amount: "12.00", reference: "order-1042" }),
});
const { payUrl } = await response.json(); // send this back to your page`;
  const page = `<script src="${origin}/widget.js" defer></script>
<button data-uniform-checkout="/api/checkout">Pay with Uniform</button>`;

  return (
    <Panel title="Add Uniform to your site">
      <Step n="1" title="Create an order on your server" code={server} />
      <Step n="2" title="Add the button to your checkout page" code={page} />
      <p className="px-5 pb-5 text-[13px] leading-[1.6] text-muted-foreground">
        The button calls your <code className="font-mono text-[12px] text-label">/api/checkout</code>, which returns the{" "}
        <code className="font-mono text-[12px] text-label">payUrl</code> from step 1. Customers pay in a card over your page. When it&apos;s paid,
        Uniform calls your webhook.{" "}
        <a href="/docs" className="text-foreground underline decoration-hair-strong underline-offset-4 hover:decoration-foreground">
          Full guide, in eight languages
        </a>
      </p>
    </Panel>
  );
}

function Step({ n, title, code }: { n: string; title: string; code: string }) {
  return (
    <div className="grid gap-3 border-b border-hair px-5 py-4">
      <p className="flex items-baseline gap-3 text-[14px] text-label">
        <span className="font-mono text-[11px] text-faint">{n}</span>
        {title}
      </p>
      <div className="relative">
        <pre className="overflow-x-auto rounded-lg border border-hair bg-black/30 p-3 pr-11 font-mono text-[12px] leading-[1.6] text-muted-foreground">
          {code}
        </pre>
        <div className="absolute top-1.5 right-1.5">
          <CopyButton text={code} label={`Copy step ${n}`} />
        </div>
      </div>
    </div>
  );
}
