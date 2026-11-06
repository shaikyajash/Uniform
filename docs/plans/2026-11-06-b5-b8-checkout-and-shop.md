# B5–B8: the cross-chain pay page, shop notifications, the shop dashboard, the widget and Demo Shop

**Goal:** a judge opens the Demo Shop, clicks Pay, logs in with an email, pays with USDT on Whitechain or USDC on Base without gas, and sees the shop's order turn paid. The shop also gets a signed webhook, a receipt link and a dashboard.

**Time:** the build plan gives B5 2 h, B6 1.5 h, B7 2.5 h, B8 1.5 h, so 7.5 h. Feature freeze is Sat 02:00 IST. Order of work: **B5 → B8 → B6 → B7**, so the demo path works first. If time runs short, the first things to cut are payment links and CSV (both B7).

## 0. Where it stands (Fri Nov 6, evening)

Sections 1–8 are the plan as approved. This section says what was built and where it differs.

- **Decisions taken:** dashboard login is (b), sign in with the payout wallet. The Demo Shop is (a), `/demo` inside the web app. Payment links were cut; CSV was built.
- **B5 pay page:** cross-chain path built (source picker, progress, returned state). Receipt page `/receipt/[orderId]` built.
- **B6 Notifier:** built. It differs from section 3 in one way: the Order Desk does not write the webhook row in the status change's transaction. The Notifier polls for orders in a final status (last 24 h) and writes one row per order and event. Body is the SPEC shape (`type`, not `status`). Retries at 10 s, 1 min, 5 min, 30 min, then failed.
  - Known gap (by design, inference): an order refunded in part gets no webhook; SPEC has no partial-refund event.
- **B7 dashboard:** wallet sign-in (challenge, signed message, 12 h session), payments with filters, CSV and refunds, the webhook log with resend, settings (webhook URL, secret rotate, API key rotate).
  - Refunds: Path A only. The payout wallet signs a USDT note; the Relayer sends `DirectPay.refund` and moves the order to `partially_refunded` or `refunded` when it is mined.
  - Not built: payment links (`/l/<id>`, `/v1/links`).
- **B8:** `/demo` shop with a server route holding the API key, and `web/public/widget.js`. `demo-shop/` stays empty.
- **Tested:**
  - Rust tests: common 29, notifier 7, order-desk 69, relayer 31. Clippy clean. Web: tsc and eslint clean.
  - Fork end to end (`server/order-desk/e2e`): wallet sign-in, webhook URL set, a Path A payment, a 0.4 then 0.6 USDT refund, a cross-chain payment and a cross-chain return. The stand-in shop server got `order.paid` ×2, `order.refunded` and `order.expired`, all with valid signatures.
  - Not yet tested: a real wallet in the browser (dashboard sign-in, refund signing, cross-chain signing on Base and Ethereum Sepolia). That needs you.

## 1. How it is today

- **Pay page** `/pay/[orderId]`: Path A only.
  - Privy email login, the USDT balance on Whitechain, one signature, and the Relayer sends it.
  - Privy is set up for Whitechain only (`supportedChains: [shopViemChain]`).
- **Order Desk:** cross-chain quote, `/legs`, `/secret`, events and refunds all work (B4, fork-tested).
- **Notifier:** an empty Rust program (`server/notifier`). There is no `webhook_log` table.
- **Dashboard, receipt page, widget, Demo Shop:** none exist. `demo-shop/` is empty.
- **Shop login:** a shop has an API key (shown once). The schema says "it signs in with its payout wallet; refunds are signed by that same wallet".
- **Refunds:** DirectPay has `refund`, signed by the shop's payout wallet. The Order Desk and Relayer have no refund flow yet. Cross-chain refunds need A9, which is not built.

## 2. B5: pay page, cross-chain path

**Flow the customer sees:**
1. Log in with email; Privy gives them a wallet.
2. "Pay with": the page reads USDT on Whitechain and USDC on Base and picks what covers the order. If both do, the customer chooses; if neither does, it shows the wallet address to top up.
3. Cross-chain:
   - **"Confirm"**: one signature makes the secret (SPEC: EIP-712, domain Uniform v1, message binding orderId and amount; `secret = keccak256(signature)`). Nothing is stored; signing again gives the same secret.
   - **"Pay 12.036 USDC"**: the second signature is the USDC note, then `POST /legs`.
4. Live progress, the same card throughout:
   - "Locking your USDC on Base";
   - "Provider is paying the shop";
   - "Paying the shop".
5. When the order is `destination_locked`, the page reads Bob's lock on Whitechain itself. It checks the merchant, the amount, the hashlock, and that the lock expires before the source lock with time to spare (SPEC). Only then does it `POST /secret`. No click needed.
6. Done: amounts, both transactions, and a receipt link. Returned: "Your USDC went back to you", with the refund transaction.

**Code:**
- `lib/chains.ts`: Base read client and USDC balance (viem).
- `lib/secret.ts`: the secret signature.
- `lib/locks.ts`: the destination lock check.
- `desk.ts`: `getQuote(…, hashlock)`, `submitLegs`, `revealSecret`.
- Privy: add Base Sepolia to `supportedChains`.
- **First check, before the rest:** that Privy's embedded wallet signs a note whose domain is chain 84532. B0 only tested Whitechain.

**Not in B5:** Ethereum Sepolia (A7) and split payments (Path C, A8). The page shows them only once `chains.json` has the addresses.

## 3. B6: notifier and receipts

- **Migration `0004_webhooks.sql`:** `webhook_log` (order, event, attempt, status code, next try, delivered).
- **Order Desk:**
  - writes one webhook row whenever an order becomes `paid`, `returned` or `expired`, in the same transaction as the status change;
  - the refund sweeper also marks unpaid orders `expired` after their time runs out.
- **Notifier (Rust, `server/notifier`):**
  - polls `webhook_log` and POSTs `{type, orderId, reference, amount, status, receipts}` to the shop's `webhook_url`;
  - signs it with `Uniform-Signature: t=<unix>,v1=<hex HMAC-SHA256(secret, "t.body")>`;
  - retries at 10 s, 1 min, 5 min, 30 min, then gives up and logs it.
- **Receipt page `/receipt/[orderId]`:** public. Shop, amount, status, the customer's transaction (Whitechain or Base) and the settlement transaction, with explorer links.

## 4. B7: shop dashboard `/dashboard`

- **Login:** the shop pastes its API key once; it is kept in the browser's session storage and sent as a Bearer token. (Recommended: see decision 1.)
- **Payments:**
  - a table of orders: date, reference, amount, path, status, customer, receipt link;
  - filter by status;
  - auto-refresh.
- **Refund (Path A only):**
  - the shop connects its payout wallet (Privy, external wallet) and signs DirectPay's refund note;
  - the Order Desk checks it and writes a `direct_refund` job;
  - the Relayer sends `DirectPay.refund`;
  - `DirectRefunded` moves the order to `partially_refunded` or `refunded`.
  - Cross-chain orders show "Refunds for cross-chain payments come later" (needs A9).
- **Payment links:** amount, memo, due date → `/l/<id>`, which makes a fresh order and opens the pay page. *(Cut first if late.)*
- **Settings:** webhook URL, webhook secret (reveal and rotate), API key rotate.
- **CSV:** `GET /v1/payments.csv`. *(Cut first if late.)*
- **Order Desk:** `GET /v1/orders` (paged), `/v1/orders/:id/refunds`, `/v1/links`, `/v1/shop` (read and update settings), `/v1/payments.csv`.
- **Relayer:** a fifth job, `direct_refund`.

## 5. B8: widget and Demo Shop

- **Widget `web/public/widget.js`:**
  - one script tag plus one button: `<script src="https://<ours>/widget.js"></script>` and `<button data-uniform-checkout="/api/checkout">`;
  - on click it calls the shop's own endpoint (which creates the order with the shop's API key, server-side), opens `/pay/<id>` in a **popup**, and listens for `postMessage({type: "uniform:paid"})`;
  - it then closes the popup and fires a `uniform:paid` DOM event on the button.
- **Why a popup, not an iframe:** Privy only works on our allowed domain, and an iframe on the shop's page would be inside the shop's page and scripts (decided earlier, B8 discussion).
- **Demo Shop:** a one-page shop ("Kettle & Co": three products, a cart, Pay):
  - a server endpoint that creates the order with the API key;
  - a webhook endpoint that verifies the signature and shows "paid (webhook verified)".
  - Where it lives: decision 2.

## 6. Look and feel

- shadcn components on the home page's tokens: dark, Geist, mono labels, hairline borders.
- No gradients-as-decoration, no emoji, no fake numbers or testimonials, no "AI" copy.
- One accent (white), state colours only for state (ok, warn, bad).
- Plain sentences: "Paying the shop", not "Processing your transaction".
- Motion only where it explains something: the card growing between steps, the progress steps.
- New shadcn components as needed: table, tabs, input, label, dialog, dropdown-menu, sonner (toasts).

## 7. Decisions for you

1. **Dashboard login.** (a) The API key, pasted once *(recommended: works now, no new auth)*. Or (b) sign in with the payout wallet: a signed message and a session, which means building wallet login. Refunds need the payout wallet's signature either way.
2. **Where the Demo Shop lives.** (a) Inside the web app at `/demo`, with a Next route holding the API key *(recommended: one deploy, fastest)*. Or (b) its own small app in `demo-shop/`, as the SPEC says. It is a different site, so it shows the widget working on someone else's site better, but it is one more thing to host.
3. **Cut list if late:** payment links and CSV first. Agree?

## 8. Not mine to do (you or A)

- **Hosting:** the deploys need your Railway and Vercel logins. I will prepare the configs and the exact commands.
- **Funding:**
  - Base Sepolia ETH for the Relayer;
  - USDC on Base for your Privy wallet;
  - a separate shop payout address.
- **A:**
  - Bob's address;
  - the Matchmaker running live against our Order Desk;
  - the SPEC change;
  - the lock id on `Settled`.
