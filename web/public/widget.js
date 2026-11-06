/*
 * Uniform checkout widget. A merchant adds one script and one button:
 *
 *   <script src="https://<uniform>/widget.js" defer></script>
 *   <button data-uniform-checkout="/api/checkout">Pay</button>
 *
 * On click, the widget POSTs to the merchant's own endpoint (which creates the order with the merchant's
 * API key, on the merchant's server) and expects `{ payUrl }` back. It opens Uniform's pay page in a
 * card over the merchant's page (an iframe in a dialog), the same in every browser: no new tab and no
 * separate window. On a phone the card fills the screen. When the order is paid the card closes by
 * itself, and the widget fires `uniform:paid` (detail: { orderId }) on the button; closing the
 * card first fires `uniform:closed`.
 *
 * The pay page runs on Uniform's own origin inside the iframe, so the merchant page's scripts cannot
 * reach the customer's login or signatures. "Paid" is only believed from that iframe and origin.
 *
 * From code: `Uniform.checkout(payUrlOrFunction)` returns a promise for { orderId }.
 * The button's request body is `data-uniform-body` (JSON text), if set.
 */
(function () {
  "use strict";
  if (window.Uniform) return;

  var script = document.currentScript;
  var ORIGIN = script ? new URL(script.src).origin : location.origin;
  var Z = 2147483000;
  // How long the pay page's "paid" screen stays up before the card closes.
  var PAID_CLOSE_MS = 1800;
  var BAR_HEIGHT = 36;

  var css =
    ".uw-veil{position:fixed;inset:0;z-index:" + Z + ";display:grid;place-items:center;padding:24px 16px;" +
    "background:rgba(5,5,6,.62);backdrop-filter:blur(3px) saturate(.6);-webkit-backdrop-filter:blur(3px) saturate(.6);" +
    "opacity:0;transition:opacity .28s cubic-bezier(.22,1,.36,1)}" +
    ".uw-veil.uw-in{opacity:1}" +
    ".uw-card{position:relative;display:flex;flex-direction:column;width:min(360px,100%);height:min(var(--uw-h,320px),100%);" +
    "overflow:hidden;border-radius:16px;background:#050506;color:#f2f2f3;" +
    "box-shadow:inset 0 1px 0 rgba(255,255,255,.12),0 0 0 1px rgba(255,255,255,.1),0 40px 80px -30px #000;" +
    "font:400 14px/1.6 ui-sans-serif,system-ui,-apple-system,'Segoe UI',sans-serif;" +
    "transform:translateY(10px) scale(.98);transition:transform .35s cubic-bezier(.22,1,.36,1),height .3s cubic-bezier(.22,1,.36,1)}" +
    ".uw-in .uw-card{transform:none}" +
    ".uw-bar{flex:none;box-sizing:border-box;display:flex;align-items:center;justify-content:space-between;height:36px;padding:0 6px 0 16px;" +
    "border-bottom:1px solid rgba(255,255,255,.08);font:400 9.5px/1 ui-monospace,SFMono-Regular,Menlo,monospace;" +
    "letter-spacing:.08em;color:#c9c9cf}" +
    ".uw-close{display:grid;place-items:center;width:26px;height:26px;border:0;border-radius:999px;background:transparent;" +
    "color:#c9c9cf;cursor:pointer;font:400 16px/1 ui-sans-serif,system-ui,sans-serif}" +
    ".uw-close:hover{background:rgba(255,255,255,.08);color:#fff}" +
    ".uw-close:focus-visible{outline:1px solid rgba(255,255,255,.6);outline-offset:1px}" +
    ".uw-body{position:relative;flex:1;min-height:0}" +
    ".uw-frame{position:absolute;inset:0;width:100%;height:100%;border:0;background:#050506;opacity:0;transition:opacity .25s ease}" +
    ".uw-ready .uw-frame{opacity:1}" +
    ".uw-wait{position:absolute;inset:0;display:grid;place-content:center;justify-items:center;gap:12px;padding:24px;" +
    "text-align:center;color:#9a9aa2;font-size:13.5px;transition:opacity .2s ease}" +
    ".uw-ready .uw-wait{opacity:0;pointer-events:none}" +
    ".uw-spin{width:18px;height:18px;border-radius:50%;border:2px solid rgba(255,255,255,.15);border-top-color:#fff;" +
    "animation:uw-spin .8s linear infinite}" +
    ".uw-failed .uw-spin{display:none}" +
    ".uw-failed .uw-wait{color:#f2f2f3}" +
    "@keyframes uw-spin{to{transform:rotate(360deg)}}" +
    "@media (max-width:560px){.uw-veil{padding:0}.uw-card{width:100%;height:100%;border-radius:0}}" +
    "@media (prefers-reduced-motion:reduce){.uw-veil,.uw-card,.uw-frame{transition:none}.uw-spin{animation-duration:2s}}";

  function addStyle() {
    if (document.getElementById("uniform-widget-style")) return;
    var style = document.createElement("style");
    style.id = "uniform-widget-style";
    style.textContent = css;
    document.head.appendChild(style);
  }

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    if (text) node.textContent = text;
    return node;
  }

  /** The dimmed page and the card. Returns controls for its states. */
  function card(onClose) {
    addStyle();
    var root = el("div", "uw-veil");
    var box = el("div", "uw-card");
    box.setAttribute("role", "dialog");
    box.setAttribute("aria-modal", "true");
    box.setAttribute("aria-label", "Pay with Uniform");
    var bar = el("div", "uw-bar");
    bar.appendChild(el("span", "", "UNIFORM · CHECKOUT"));
    var close = el("button", "uw-close", "×");
    close.type = "button";
    close.setAttribute("aria-label", "Close checkout");
    bar.appendChild(close);
    var body = el("div", "uw-body");
    var wait = el("div", "uw-wait");
    wait.appendChild(el("span", "uw-spin"));
    var waitText = el("span", "", "Starting checkout…");
    wait.appendChild(waitText);
    var frame = el("iframe", "uw-frame");
    // aria-label, not title: Safari shows a title as a tooltip over the whole pay page.
    frame.setAttribute("aria-label", "Uniform checkout");
    frame.setAttribute("allow", "clipboard-write");
    body.appendChild(wait);
    body.appendChild(frame);
    box.appendChild(bar);
    box.appendChild(body);
    root.appendChild(box);

    // The merchant's page stays put underneath; it must not scroll behind the card.
    var html = document.documentElement;
    var overflow = html.style.overflow;
    html.style.overflow = "hidden";
    var opener = document.activeElement;

    function onKey(event) {
      if (event.key === "Escape") onClose();
    }
    close.addEventListener("click", onClose);
    document.addEventListener("keydown", onKey);
    document.body.appendChild(root);
    requestAnimationFrame(function () {
      root.classList.add("uw-in");
    });
    close.focus();

    return {
      frame: frame,
      /** The pay page's own height (it reports it); the bar is added on top. */
      size: function (height) {
        if (typeof height === "number" && height > 0) box.style.setProperty("--uw-h", Math.ceil(height + BAR_HEIGHT) + "px");
      },
      load: function (payUrl) {
        frame.addEventListener("load", function () {
          root.classList.add("uw-ready");
          frame.focus();
        });
        // The pay page sees it is in a frame and drops its own nav (web/app/layout.tsx).
        frame.src = payUrl;
      },
      fail: function (message) {
        root.classList.add("uw-failed");
        waitText.textContent = message;
      },
      remove: function (delay) {
        setTimeout(function () {
          document.removeEventListener("keydown", onKey);
          root.classList.remove("uw-in");
          setTimeout(function () {
            root.remove();
            html.style.overflow = overflow;
            if (opener && opener.focus) opener.focus();
          }, 300);
        }, delay || 0);
      },
    };
  }

  /**
   * Opens the pay page in the card and resolves with { orderId } once paid; rejects with
   * Error("closed") if the customer closes it first. `target` is a pay URL, or a function returning
   * (a promise for) one.
   */
  function checkout(target) {
    return new Promise(function (resolve, reject) {
      var done = false;
      var ui = card(function () {
        finish(false);
      });

      function finish(paid, orderId) {
        if (done) return;
        done = true;
        window.removeEventListener("message", onMessage);
        if (paid) {
          ui.remove(PAID_CLOSE_MS);
          resolve({ orderId: orderId });
        } else {
          ui.remove(0);
          reject(new Error("closed"));
        }
      }

      function onMessage(event) {
        if (event.origin !== ORIGIN || event.source !== ui.frame.contentWindow) return;
        if (!event.data) return;
        if (event.data.type === "uniform:size") ui.size(event.data.height);
        if (event.data.type === "uniform:paid") finish(true, event.data.orderId);
      }
      window.addEventListener("message", onMessage);

      Promise.resolve(typeof target === "function" ? target() : target).then(
        function (payUrl) {
          if (!done) ui.load(payUrl);
        },
        function (error) {
          if (done) return;
          ui.fail((error && error.message) || "The merchant couldn't create the order.");
        },
      );
    });
  }

  /** `<button data-uniform-checkout="/endpoint">`: POST there for { payUrl }, then check out. */
  function bind(button) {
    if (button.__uniform) return;
    button.__uniform = true;
    button.addEventListener("click", function () {
      var endpoint = button.getAttribute("data-uniform-checkout");
      var body = button.getAttribute("data-uniform-body") || "{}";
      button.disabled = true;
      checkout(function () {
        return fetch(endpoint, { method: "POST", headers: { "content-type": "application/json" }, body: body })
          .then(function (response) {
            return response.json().then(function (json) {
              if (!response.ok || !json.payUrl) throw new Error(json.error || "The merchant couldn't create the order.");
              return json.payUrl;
            });
          });
      }).then(
        function (result) {
          button.disabled = false;
          button.dispatchEvent(new CustomEvent("uniform:paid", { bubbles: true, detail: result }));
        },
        function () {
          button.disabled = false;
          button.dispatchEvent(new CustomEvent("uniform:closed", { bubbles: true }));
        },
      );
    });
  }

  function bindAll() {
    document.querySelectorAll("[data-uniform-checkout]").forEach(bind);
  }

  window.Uniform = { checkout: checkout, bind: bindAll };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", bindAll);
  else bindAll();
})();
