import type { Metadata } from "next";
import { headers } from "next/headers";
import type { ReactNode } from "react";
import { Frame } from "@/app/_ui/frame";
import { CodeBlock, CodeTabs } from "./code";
import {
  ERROR_BODY,
  LANGS,
  LANG_GRAMMAR,
  LANG_LABEL,
  ORDER_CREATED,
  WEBHOOK_BODY,
  createOrder,
  readOrder,
  verifyWebhook,
  widget,
  type Lang,
} from "./samples";
import "./docs.css";

export const metadata: Metadata = {
  title: "Docs · Uniform",
  description: "Take stablecoin payments with Uniform: create an order, show the checkout, trust the webhook.",
};

const SECTIONS = [
  { id: "overview", label: "Overview" },
  { id: "keys", label: "Get your keys" },
  { id: "create", label: "Create an order" },
  { id: "checkout", label: "Show the checkout" },
  { id: "webhooks", label: "Webhooks" },
  { id: "read", label: "Read an order" },
  { id: "statuses", label: "Order statuses" },
  { id: "errors", label: "Errors" },
  { id: "testing", label: "Testing" },
];

/** The samples name this site's own address, read from the request, so the page renders per request. */
export const instant = false;

/** `/docs`: how a developer adds Uniform to a store, with every sample in eight languages. */
export default async function DocsPage() {
  const list = await headers();
  const host = list.get("x-forwarded-host") ?? list.get("host") ?? "localhost:3000";
  const proto = list.get("x-forwarded-proto") ?? (host.startsWith("localhost") ? "http" : "https");
  const site = `${proto}://${host}`;
  const base = `${site}/desk/v1`;

  const tabs = (byLang: Record<Lang, string>) =>
    LANGS.map((lang) => ({ key: lang, label: LANG_LABEL[lang], lang: LANG_GRAMMAR[lang], code: byLang[lang] }));

  return (
    <main className="relative min-h-screen overflow-x-clip px-[calc(var(--gutter)+20px)] pt-[max(120px,14vh)] pb-24">
      <Frame />
      <div className="mx-auto grid max-w-[1080px] gap-12 lg:grid-cols-[180px_minmax(0,1fr)]">
        <nav aria-label="On this page" className="hidden lg:block">
          <ul className="sticky top-28 grid gap-0.5 border-l border-hair">
            {SECTIONS.map((section) => (
              <li key={section.id}>
                <a
                  href={`#${section.id}`}
                  className="-ml-px block border-l border-transparent py-1.5 pl-4 text-[13px] text-muted-foreground transition-colors hover:border-foreground hover:text-foreground"
                >
                  {section.label}
                </a>
              </li>
            ))}
          </ul>
        </nav>

        <article className="docs min-w-0 max-w-[740px]">
          <header className="mb-14">
            <p className="font-mono text-[11px] tracking-[0.12em] text-faint uppercase">Developers</p>
            <h1 className="mt-3 text-[clamp(34px,4.4vw,52px)] leading-[1.05] font-normal tracking-[-0.035em]">
              Take stablecoin payments
              <span className="block text-muted-foreground">in three calls.</span>
            </h1>
            <p className="mt-5 max-w-[560px] text-[15.5px] leading-[1.7] text-muted-foreground">
              Your server creates an order. Your page opens the checkout. Uniform tells your server when it&apos;s paid.
              You always receive USDT on Whitechain, whatever the customer paid with.
            </p>
            <dl className="mt-8 grid gap-px overflow-hidden rounded-md border border-hair bg-hair sm:grid-cols-2">
              <Fact label="API base">
                <code>{base}</code>
              </Fact>
              <Fact label="Widget">
                <code>{site}/widget.js</code>
              </Fact>
            </dl>
          </header>

          <Section id="overview" title="Overview">
            <ol className="docs-steps">
              <li>
                <b>Your server</b> calls <Code>POST /v1/orders</Code> with your API key and gets a <Code>payUrl</Code>.
              </li>
              <li>
                <b>Your page</b> opens that URL in Uniform&apos;s checkout card. The customer logs in with an email and pays with USDT
                on Whitechain or USDC on Base or Ethereum. They pay no gas.
              </li>
              <li>
                <b>Uniform</b> posts a signed <Code>order.paid</Code> webhook to your server. Ship the order when it arrives.
              </li>
            </ol>
            <p>
              Your API key never reaches the browser, and the browser never decides that an order is paid. Only the signed webhook,
              or reading the order yourself, does.
            </p>
          </Section>

          <Section id="keys" title="Get your keys">
            <p>
              Open the <a href="/dashboard">dashboard</a> and connect the wallet you want to be paid to. If it isn&apos;t a merchant
              yet, give your business a name and sign one free message. You get two secrets, shown once:
            </p>
            <table>
              <tbody>
                <tr>
                  <td>
                    <Code>API key</Code>
                  </td>
                  <td>Creates orders. Send it as <Code>Authorization: Bearer …</Code>. Server only.</td>
                </tr>
                <tr>
                  <td>
                    <Code>Webhook secret</Code>
                  </td>
                  <td>Starts with <Code>whsec_</Code>. Checks that a webhook really came from Uniform.</td>
                </tr>
              </tbody>
            </table>
            <p>Lost one? Replace it in Settings. The old one stops working at once.</p>
          </Section>

          <Section id="create" title="Create an order">
            <Endpoint method="POST" path="/v1/orders" auth="API key" />
            <p>Call this from your server when the customer clicks pay.</p>
            <table>
              <thead>
                <tr>
                  <th>Field</th>
                  <th>Type</th>
                  <th>Notes</th>
                </tr>
              </thead>
              <tbody>
                <tr>
                  <td>
                    <Code>amount</Code>
                  </td>
                  <td>string</td>
                  <td>USDT as decimal text, up to 6 decimals: <Code>&quot;12&quot;</Code>, <Code>&quot;12.50&quot;</Code>. Above zero, at most 100.</td>
                </tr>
                <tr>
                  <td>
                    <Code>reference</Code>
                  </td>
                  <td>string</td>
                  <td>Your own id for the sale, 1 to 200 characters. Comes back in every webhook.</td>
                </tr>
                <tr>
                  <td>
                    <Code>successUrl</Code>
                  </td>
                  <td>string, optional</td>
                  <td>Where to send the customer after paying. <Code>https://</Code> or <Code>http://</Code>.</td>
                </tr>
              </tbody>
            </table>
            <CodeTabs variants={tabs(createOrder(base))} />
            <p>
              The answer is <Code>201 Created</Code>. The customer has 30 minutes to pay before the order expires.
            </p>
            <CodeBlock code={ORDER_CREATED} lang="json" title="201 Created" />
          </Section>

          <Section id="checkout" title="Show the checkout">
            <p>
              Add the widget and point a button at an endpoint on your server that creates the order and answers{" "}
              <Code>{"{ payUrl }"}</Code>. The checkout opens in a card over your page, the same in every browser. It closes by
              itself once paid.
            </p>
            <CodeTabs variants={widget(site).map((sample) => ({ key: sample.label.toLowerCase(), label: sample.label, lang: sample.lang, code: sample.code }))} />
            <table>
              <tbody>
                <tr>
                  <td>
                    <Code>uniform:paid</Code>
                  </td>
                  <td>Fired on the button once paid. <Code>event.detail.orderId</Code> is the order.</td>
                </tr>
                <tr>
                  <td>
                    <Code>uniform:closed</Code>
                  </td>
                  <td>The customer closed the card first. <Code>Uniform.checkout()</Code> rejects with <Code>Error(&quot;closed&quot;)</Code>.</td>
                </tr>
              </tbody>
            </table>
            <Note>
              Treat <Code>uniform:paid</Code> as a hint for your page, not as proof. Ship on the webhook.
            </Note>
            <p>
              No JavaScript? Redirect the customer to <Code>payUrl</Code> instead. It works as a full page too.
            </p>
          </Section>

          <Section id="webhooks" title="Webhooks">
            <p>
              Set your webhook URL in the dashboard under Settings. Uniform sends a <Code>POST</Code> with a JSON body when an order
              ends:
            </p>
            <table>
              <tbody>
                <tr>
                  <td>
                    <Code>order.paid</Code>
                  </td>
                  <td>You have the USDT. Ship the order.</td>
                </tr>
                <tr>
                  <td>
                    <Code>order.expired</Code>
                  </td>
                  <td>Nobody paid in time, or a cross-chain payment ran out and went back to the customer.</td>
                </tr>
                <tr>
                  <td>
                    <Code>order.refunded</Code>
                  </td>
                  <td>You refunded the full amount from the dashboard.</td>
                </tr>
              </tbody>
            </table>
            <CodeBlock code={WEBHOOK_BODY} lang="http" title="What your server receives" />
            <h3>Check the signature</h3>
            <p>
              <Code>X-Uniform-Signature</Code> is the HMAC-SHA256 of the raw body, keyed with your webhook secret, as lowercase hex.
              Compute it over the bytes you received, before parsing, and compare in constant time.
            </p>
            <CodeTabs variants={tabs(verifyWebhook())} />
            <h3>Delivery</h3>
            <p>
              Answer with any <Code>2xx</Code> within 10 seconds. Anything else is tried again after 10 seconds, 1 minute, 5 minutes
              and 30 minutes, then given up. You can resend any delivery from the dashboard&apos;s Webhooks tab. A delivery can arrive
              twice, so make your handler safe to run again for the same <Code>orderId</Code>.
            </p>
          </Section>

          <Section id="read" title="Read an order">
            <Endpoint method="GET" path="/v1/orders/{orderId}" auth="none" />
            <p>
              Anyone with the order id can read it; the id is random and unguessable. Use it to check an order&apos;s state yourself,
              for example on your thank-you page.
            </p>
            <CodeTabs variants={tabs(readOrder(base))} />
          </Section>

          <Section id="statuses" title="Order statuses">
            <table>
              <tbody>
                {[
                  ["created", "Made by your server. Nobody has paid."],
                  ["quoted", "The customer opened the checkout and got a price."],
                  ["paying", "The customer signed; the payment is on its way."],
                  ["source_locked", "Cross-chain: the customer's USDC is locked on their chain."],
                  ["destination_locked", "Cross-chain: the provider's USDT is locked for you on Whitechain."],
                  ["paid", "You have the USDT."],
                  ["partially_refunded", "You gave back part of it."],
                  ["refunded", "You gave back all of it."],
                  ["expired", "Nobody paid in time. Nothing moved."],
                  ["returned", "Cross-chain ran out of time; the customer's money went back to them."],
                ].map(([status, meaning]) => (
                  <tr key={status}>
                    <td>
                      <Code>{status}</Code>
                    </td>
                    <td>{meaning}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </Section>

          <Section id="errors" title="Errors">
            <p>Every error has the same shape and a matching HTTP status:</p>
            <CodeBlock code={ERROR_BODY} lang="http" />
            <table>
              <tbody>
                {[
                  ["401", "unauthorized", "Missing or wrong API key."],
                  ["400", "bad_amount", "Not decimal USDT, more than 6 decimals, or zero."],
                  ["400", "amount_too_large", "Above the per-order limit (100 USDT)."],
                  ["400", "bad_reference", "Empty or longer than 200 characters."],
                  ["400", "bad_success_url", "Not an http:// or https:// address."],
                  ["404", "not_found", "No order with that id."],
                ].map(([status, code, meaning]) => (
                  <tr key={code}>
                    <td className="font-mono text-faint">{status}</td>
                    <td>
                      <Code>{code}</Code>
                    </td>
                    <td>{meaning}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </Section>

          <Section id="testing" title="Testing">
            <p>
              Uniform runs on test networks today: Whitechain testnet, Base Sepolia and Ethereum Sepolia. Payments use test USDT and
              USDC with no real value. Free test USDC is at{" "}
              <a href="https://faucet.circle.com" target="_blank" rel="noreferrer">
                faucet.circle.com
              </a>
              .
            </p>
            <p>
              Want to see it whole first? The <a href="/demo">demo store</a> uses exactly the calls above.
            </p>
          </Section>
        </article>
      </div>
    </main>
  );
}

function Section({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return (
    <section id={id} className="scroll-mt-28 border-t border-hair pt-10 pb-6">
      <h2>{title}</h2>
      {children}
    </section>
  );
}

function Code({ children }: { children: ReactNode }) {
  return <code className="docs-inline">{children}</code>;
}

function Endpoint({ method, path, auth }: { method: string; path: string; auth: string }) {
  return (
    <p className="docs-endpoint">
      <span className="docs-method">{method}</span>
      <code>{path}</code>
      <span className="docs-auth">Auth: {auth}</span>
    </p>
  );
}

function Note({ children }: { children: ReactNode }) {
  return <p className="docs-note">{children}</p>;
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid gap-1.5 bg-background px-4 py-3">
      <dt className="font-mono text-[10.5px] tracking-[0.12em] text-faint uppercase">{label}</dt>
      <dd className="min-w-0 truncate font-mono text-[12.5px] text-label">{children}</dd>
    </div>
  );
}
