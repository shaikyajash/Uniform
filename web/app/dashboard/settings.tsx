"use client";

// The merchant's settings: where webhooks go, the secret that signs them, and the API key.

import { useState, type ReactNode } from "react";
import { Check, Copy, Eye, EyeOff, Loader2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  explainDashboard,
  rotateApiKey,
  rotateWebhookSecret,
  updateShop,
  type Session,
  type Settings as ShopSettings,
} from "@/lib/dashboard";
import { Eyebrow, Panel } from "./bits";
import { Integrate } from "./integrate";

export function Settings({
  session,
  settings,
  onChange,
}: {
  session: Session;
  settings: ShopSettings | null;
  onChange: (settings: ShopSettings) => void;
}) {
  if (!settings) {
    return (
      <div className="grid place-items-center py-16">
        <Loader2 className="size-4 animate-spin text-faint" />
      </div>
    );
  }
  return (
    <div className="grid gap-6">
      <Integrate />
      <Panel title="Merchant">
        <Row label="Name">{settings.name}</Row>
        <Row label="Paid to">
          <span className="font-mono text-[13px]">{settings.payoutAddress}</span>
        </Row>
        <Row label="Currency">USDT on Whitechain</Row>
      </Panel>
      <Panel title="Webhooks">
        <WebhookUrl session={session} settings={settings} onChange={onChange} />
        <WebhookSecret session={session} settings={settings} onChange={onChange} />
      </Panel>
      <Panel title="API key">
        <ApiKey session={session} />
      </Panel>
    </div>
  );
}

function Row({ label, children, hint }: { label: string; children: ReactNode; hint?: ReactNode }) {
  return (
    <div className="grid gap-3 border-b border-hair px-5 py-4 last:border-b-0 sm:grid-cols-[180px_1fr] sm:items-center">
      <div className="grid gap-1">
        <Eyebrow>{label}</Eyebrow>
        {hint && <p className="text-[12px] leading-[1.5] text-faint">{hint}</p>}
      </div>
      <div className="min-w-0 text-[14px] text-label">{children}</div>
    </div>
  );
}

function WebhookUrl({ session, settings, onChange }: { session: Session; settings: ShopSettings; onChange: (s: ShopSettings) => void }) {
  const [url, setUrl] = useState(settings.webhookUrl ?? "");
  const [busy, setBusy] = useState(false);
  const changed = url.trim() !== (settings.webhookUrl ?? "");
  return (
    <Row label="URL" hint="Uniform POSTs paid, refunded and expired orders here.">
      <form
        className="flex gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          setBusy(true);
          updateShop(session.token, url.trim() || null)
            .then((next) => {
              onChange(next);
              toast.success(next.webhookUrl ? "Webhook URL saved" : "Webhooks turned off");
            })
            .catch((error) => toast.error(explainDashboard(error)))
            .finally(() => setBusy(false));
        }}
      >
        <Input
          type="url"
          placeholder="https://your-site.com/uniform/webhook"
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          className="h-10 border-hair-strong bg-black/30 font-mono text-[13px]"
        />
        <Button type="submit" disabled={!changed || busy} className="h-10 rounded-full bg-white px-5 text-black hover:bg-white/90">
          {busy ? <Loader2 className="animate-spin" /> : "Save"}
        </Button>
      </form>
    </Row>
  );
}

function WebhookSecret({ session, settings, onChange }: { session: Session; settings: ShopSettings; onChange: (s: ShopSettings) => void }) {
  const [shown, setShown] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const secret = settings.webhookSecret;
  return (
    <Row label="Signing secret" hint="Check X-Uniform-Signature with it: hex HMAC-SHA256 of the body.">
      <div className="flex flex-wrap items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded-lg border border-hair bg-black/30 px-3 py-2.5 font-mono text-[12.5px]">
          {shown ? secret : `${secret.slice(0, 6)}${"•".repeat(24)}`}
        </code>
        <Button variant="ghost" size="icon-sm" aria-label={shown ? "Hide secret" : "Show secret"} onClick={() => setShown(!shown)}>
          {shown ? <EyeOff /> : <Eye />}
        </Button>
        <CopyButton text={secret} label="Copy secret" />
        <Button variant="ghost" size="sm" className="text-muted-foreground hover:text-foreground" onClick={() => setConfirm(true)}>
          Replace
        </Button>
      </div>
      <Confirm
        open={confirm}
        busy={busy}
        title="Replace the signing secret?"
        body="Webhooks are signed with the new secret from now on, including ones still being retried. Update your server right after."
        action="Replace secret"
        onCancel={() => setConfirm(false)}
        onConfirm={() => {
          setBusy(true);
          rotateWebhookSecret(session.token)
            .then(({ secret: next }) => {
              onChange({ ...settings, webhookSecret: next });
              setShown(true);
              setConfirm(false);
              toast.success("New signing secret ready");
            })
            .catch((error) => toast.error(explainDashboard(error)))
            .finally(() => setBusy(false));
        }}
      />
    </Row>
  );
}

function ApiKey({ session }: { session: Session }) {
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [fresh, setFresh] = useState<string | null>(null);
  return (
    <Row label="Secret key" hint="Your server uses it to create orders. Only its hash is stored, so it can't be shown again.">
      <div className="flex flex-wrap items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded-lg border border-hair bg-black/30 px-3 py-2.5 font-mono text-[12.5px] text-muted-foreground">
          sk_test_{"•".repeat(24)}
        </code>
        <Button variant="ghost" size="sm" className="text-muted-foreground hover:text-foreground" onClick={() => setConfirm(true)}>
          Replace
        </Button>
      </div>
      <Confirm
        open={confirm}
        busy={busy}
        title="Replace the API key?"
        body="The current key stops working at once. Orders your server creates fail until it has the new key."
        action="Replace key"
        onCancel={() => setConfirm(false)}
        onConfirm={() => {
          setBusy(true);
          rotateApiKey(session.token)
            .then(({ secret }) => {
              setConfirm(false);
              setFresh(secret);
            })
            .catch((error) => toast.error(explainDashboard(error)))
            .finally(() => setBusy(false));
        }}
      />
      <Dialog open={fresh !== null} onOpenChange={(open) => !open && setFresh(null)}>
        <DialogContent className="border-hair-strong bg-gradient-to-b from-panel-2 to-panel sm:max-w-[480px]">
          <DialogHeader>
            <Eyebrow>Shown once</Eyebrow>
            <DialogTitle className="font-display text-2xl font-normal tracking-[-0.02em]">Your new API key</DialogTitle>
            <DialogDescription>Copy it into your server now. Uniform keeps only a hash and can&apos;t show it again.</DialogDescription>
          </DialogHeader>
          <div className="flex items-center gap-2">
            <code className="min-w-0 flex-1 rounded-lg border border-hair-strong bg-black/40 px-3 py-3 font-mono text-[12px] break-all text-foreground">
              {fresh}
            </code>
            {fresh && <CopyButton text={fresh} label="Copy API key" />}
          </div>
          <DialogFooter>
            <Button className="h-10 w-full rounded-full bg-white text-black hover:bg-white/90" onClick={() => setFresh(null)}>
              I&apos;ve saved it
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </Row>
  );
}

export function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      variant="ghost"
      size="icon-sm"
      aria-label={label}
      onClick={() =>
        void navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        })
      }
    >
      {copied ? <Check className="text-ok" /> : <Copy />}
    </Button>
  );
}

function Confirm({
  open,
  busy,
  title,
  body,
  action,
  onCancel,
  onConfirm,
}: {
  open: boolean;
  busy: boolean;
  title: string;
  body: string;
  action: string;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={(next) => !next && !busy && onCancel()}>
      <DialogContent className="border-hair-strong bg-gradient-to-b from-panel-2 to-panel sm:max-w-[420px]">
        <DialogHeader>
          <DialogTitle className="font-display text-xl font-normal tracking-[-0.02em]">{title}</DialogTitle>
          <DialogDescription className="leading-[1.65]">{body}</DialogDescription>
        </DialogHeader>
        <DialogFooter className="gap-2 sm:gap-2">
          <Button variant="ghost" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button className="rounded-full bg-white px-5 text-black hover:bg-white/90" onClick={onConfirm} disabled={busy}>
            {busy ? <Loader2 className="animate-spin" /> : null}
            {action}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
