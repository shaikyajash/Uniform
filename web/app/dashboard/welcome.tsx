"use client";

// Right after sign-up: the new merchant's API key and webhook secret, shown this once.

import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import type { Created } from "@/lib/dashboard";
import { Eyebrow } from "./bits";
import { CopyButton } from "./settings";

export function Welcome({ created, name }: { created: Created; name: string }) {
  const [open, setOpen] = useState(true);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="border-hair-strong bg-gradient-to-b from-panel-2 to-panel sm:max-w-[520px]">
        <DialogHeader>
          <Eyebrow>Shown once</Eyebrow>
          <DialogTitle className="font-display text-2xl font-normal tracking-[-0.02em]">{name} is ready</DialogTitle>
          <DialogDescription>
            Save these two now. Uniform can&apos;t show them again; you can replace them in Settings.
          </DialogDescription>
        </DialogHeader>
        <Secret label="API key" hint="Your server uses it to create orders." value={created.apiKey} />
        <Secret label="Webhook secret" hint="Checks that a webhook really came from Uniform." value={created.webhookSecret} />
        <DialogFooter>
          <Button className="h-10 w-full rounded-full bg-white text-black hover:bg-white/90" onClick={() => setOpen(false)}>
            I&apos;ve saved them
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function Secret({ label, hint, value }: { label: string; hint: string; value: string }) {
  return (
    <div className="grid gap-2">
      <div className="flex items-baseline justify-between gap-3">
        <Eyebrow>{label}</Eyebrow>
        <span className="text-[12px] text-faint">{hint}</span>
      </div>
      <div className="flex items-center gap-2">
        <code className="min-w-0 flex-1 rounded-lg border border-hair-strong bg-black/40 px-3 py-2.5 font-mono text-[12px] break-all text-foreground">
          {value}
        </code>
        <CopyButton text={value} label={`Copy ${label}`} />
      </div>
    </div>
  );
}
