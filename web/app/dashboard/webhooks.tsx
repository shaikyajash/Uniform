"use client";

// The webhook log: every message Uniform sent (or is trying to send) to the merchant's server.

import { useState } from "react";
import { Loader2, RotateCw } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { explainDashboard, listWebhooks, retryWebhook, type Delivery, type Session } from "@/lib/dashboard";
import { cn } from "@/lib/utils";
import { Dot, Eyebrow, Panel, formatTime, usePolling } from "./bits";

const REFRESH_MS = 4000;

export function Webhooks({ session, webhookUrl, onSetUp }: { session: Session; webhookUrl: string | null; onSetUp: () => void }) {
  const [deliveries, reload] = usePolling(() => listWebhooks(session.token), REFRESH_MS, [session.token]);
  const [open, setOpen] = useState<Delivery | null>(null);

  return (
    <Panel
      title="Webhooks"
      action={
        webhookUrl ? (
          <span className="max-w-[60%] truncate font-mono text-[11.5px] text-muted-foreground" title={webhookUrl}>
            {webhookUrl}
          </span>
        ) : null
      }
    >
      {!deliveries ? (
        <div className="grid place-items-center py-16">
          <Loader2 className="size-4 animate-spin text-faint" />
        </div>
      ) : deliveries.length === 0 ? (
        <div className="grid justify-items-center gap-2 px-6 py-16 text-center">
          <p className="text-[14.5px]">{webhookUrl ? "No webhooks sent yet" : "Webhooks are off"}</p>
          <p className="max-w-sm text-[13px] leading-[1.6] text-muted-foreground">
            {webhookUrl
              ? "When an order is paid, refunded or expires, Uniform tells your server here."
              : "Add your server's URL and Uniform will tell it the moment an order is paid, refunded or expires."}
          </p>
          {!webhookUrl && (
            <Button variant="outline" size="sm" className="mt-2 rounded-full border-hair-strong" onClick={onSetUp}>
              Add a webhook URL
            </Button>
          )}
        </div>
      ) : (
        <Table>
          <TableHeader>
            <TableRow className="border-hair hover:bg-transparent">
              {["Time", "Event", "Order", "Result", ""].map((head, index) => (
                <TableHead
                  key={index}
                  className="h-10 font-mono text-[10px] font-normal tracking-[0.1em] text-faint uppercase first:pl-5 last:pr-5"
                >
                  {head}
                </TableHead>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>
            {deliveries.map((delivery) => (
              <TableRow
                key={delivery.id}
                className="cursor-pointer border-hair text-[13.5px] hover:bg-white/[0.02]"
                onClick={() => setOpen(delivery)}
              >
                <TableCell className="pl-5 text-muted-foreground tabular-nums">{formatTime(delivery.createdAt)}</TableCell>
                <TableCell className="font-mono text-[12.5px]">{delivery.event}</TableCell>
                <TableCell className="max-w-[160px] truncate font-mono text-[12.5px] text-muted-foreground">
                  {delivery.reference}
                </TableCell>
                <TableCell>
                  <Result delivery={delivery} />
                </TableCell>
                <TableCell className="pr-5 text-right">
                  {delivery.status !== "pending" && (
                    <ResendButton token={session.token} delivery={delivery} onDone={reload} />
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      <Dialog open={open !== null} onOpenChange={(next) => !next && setOpen(null)}>
        <DialogContent className="border-hair-strong bg-gradient-to-b from-panel-2 to-panel sm:max-w-[560px]">
          <DialogHeader>
            <Eyebrow>{open?.event}</Eyebrow>
            <DialogTitle className="font-display text-xl font-normal tracking-[-0.02em]">What was sent</DialogTitle>
            <DialogDescription>
              Signed with your webhook secret in <code className="font-mono text-[12px]">X-Uniform-Signature</code>.
            </DialogDescription>
          </DialogHeader>
          <pre className="max-h-[50vh] overflow-auto rounded-xl border border-hair bg-black/40 p-4 font-mono text-[12px] leading-[1.7] text-label">
            {open ? JSON.stringify(open.body, null, 2) : ""}
          </pre>
          {open?.lastError && <p className="text-[12.5px] text-bad">Last reply: {open.lastError}</p>}
        </DialogContent>
      </Dialog>
    </Panel>
  );
}

function Result({ delivery }: { delivery: Delivery }) {
  const tone = delivery.status === "delivered" ? "ok" : delivery.status === "failed" ? "bad" : "live";
  const text =
    delivery.status === "delivered"
      ? `Delivered${delivery.lastStatus ? ` · ${delivery.lastStatus}` : ""}`
      : delivery.status === "failed"
        ? `Failed after ${delivery.attempts} tries`
        : delivery.attempts === 0
          ? "Sending"
          : `Retrying · try ${delivery.attempts + 1}`;
  return (
    <span className={cn("inline-flex items-center gap-2", delivery.status === "failed" && "text-bad")}>
      <Dot tone={tone} />
      {text}
    </span>
  );
}

function ResendButton({ token, delivery, onDone }: { token: string; delivery: Delivery; onDone: () => void }) {
  const [busy, setBusy] = useState(false);
  return (
    <Button
      variant="ghost"
      size="sm"
      disabled={busy}
      className="text-muted-foreground hover:text-foreground"
      onClick={(event) => {
        event.stopPropagation();
        setBusy(true);
        retryWebhook(token, delivery.id)
          .then(() => {
            toast("Sending again", { description: `${delivery.event} for ${delivery.reference}` });
            onDone();
          })
          .catch((error) => toast.error(explainDashboard(error)))
          .finally(() => setBusy(false));
      }}
    >
      <RotateCw className={cn(busy && "animate-spin")} /> Send again
    </Button>
  );
}
