"use client";

// Log in with email in our own form, on the full pay page and in the widget: Privy sends and checks
// the code, but no Privy window opens. Email first, then the 6-digit code.

import { useLoginWithEmail } from "@privy-io/react-auth";
import { useState } from "react";
import { Input } from "@/components/ui/input";
import { FinePrint, PrimaryButton, Problem, TextButton } from "./parts";

const CODE_LENGTH = 6;
const LOOKS_LIKE_EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export function EmailLogin() {
  const { sendCode, loginWithCode, state } = useLoginWithEmail();
  const [email, setEmail] = useState("");
  const [code, setCode] = useState("");
  const [sentTo, setSentTo] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const sending = state.status === "sending-code";
  const checking = state.status === "submitting-code";

  function send(to: string) {
    setProblem(null);
    sendCode({ email: to }).then(
      () => setSentTo(to),
      () => setProblem("Couldn't send the code. Check the email and try again."),
    );
  }

  if (sentTo === null) {
    const valid = LOOKS_LIKE_EMAIL.test(email.trim());
    return (
      <form
        className="grid gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (valid && !sending) send(email.trim());
        }}
      >
        <label className="grid gap-1.5">
          <span className="text-[12px] text-muted-foreground">Email</span>
          <Input
            type="email"
            autoComplete="email"
            placeholder="you@example.com"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
            className="h-9 border-hair-strong bg-black/30 text-[13px]"
          />
        </label>
        {problem && <Problem>{problem}</Problem>}
        <PrimaryButton type="submit" disabled={!valid || sending}>
          {sending ? "Sending code…" : "Continue"}
        </PrimaryButton>
        <FinePrint>We create a wallet for you. Nothing to install.</FinePrint>
      </form>
    );
  }

  return (
    <form
      className="grid gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (code.length !== CODE_LENGTH || checking) return;
        setProblem(null);
        loginWithCode({ code }).catch(() => {
          setCode("");
          setProblem("That code didn't work. Check it, or send a new one.");
        });
      }}
    >
      <label className="grid gap-1.5">
        <span className="text-[12px] text-muted-foreground">
          Code sent to <span className="text-foreground">{sentTo}</span>
        </span>
        <Input
          autoFocus
          inputMode="numeric"
          autoComplete="one-time-code"
          placeholder="6-digit code"
          maxLength={CODE_LENGTH}
          value={code}
          onChange={(event) => setCode(event.target.value.replace(/\D/g, "").slice(0, CODE_LENGTH))}
          className="h-9 border-hair-strong bg-black/30 font-mono text-[14px] tracking-[0.3em]"
        />
      </label>
      {problem && <Problem>{problem}</Problem>}
      <PrimaryButton type="submit" disabled={code.length !== CODE_LENGTH || checking}>
        {checking ? "Checking…" : "Log in"}
      </PrimaryButton>
      <div className="flex justify-between">
        <TextButton
          type="button"
          onClick={() => {
            setSentTo(null);
            setCode("");
            setProblem(null);
          }}
        >
          Other email
        </TextButton>
        <TextButton type="button" disabled={sending} onClick={() => send(sentTo)}>
          {sending ? "Sending…" : "Send again"}
        </TextButton>
      </div>
    </form>
  );
}
