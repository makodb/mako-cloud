import { Button } from "@mako-cloud/ui";
import { KeyRound } from "lucide-react";
import { useEffect, useRef, useState } from "react";

const AUTO_DISMISS_MILLISECONDS = 5 * 60 * 1_000;

export function confirmDestructiveAction({
  action,
  target,
  consequence,
}: {
  readonly action: string;
  readonly target: string;
  readonly consequence: string;
}): boolean {
  return window.confirm(`${action} ${target}?\n\n${consequence}\n\nThis action will be audited.`);
}

export function OneTimeSecretValue({
  label,
  value,
  onDismiss,
}: {
  readonly label: string;
  readonly value: string;
  readonly onDismiss: () => void;
}) {
  const [revealed, setRevealed] = useState(false);
  const [copyStatus, setCopyStatus] = useState("");
  const heading = useRef<HTMLElement>(null);
  const dismiss = useRef(onDismiss);
  dismiss.current = onDismiss;

  useEffect(() => {
    if (value.length === 0) {
      dismiss.current();
      return;
    }
    setRevealed(false);
    setCopyStatus("");
    heading.current?.focus();
    const timer = window.setTimeout(() => dismiss.current(), AUTO_DISMISS_MILLISECONDS);
    const hideWhenBackgrounded = () => {
      if (document.visibilityState === "hidden") {
        setRevealed(false);
      }
    };
    document.addEventListener("visibilitychange", hideWhenBackgrounded);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", hideWhenBackgrounded);
    };
  }, [value]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopyStatus("Copied. Clear your clipboard after storing the value securely.");
    } catch {
      setCopyStatus("Clipboard access was unavailable. Reveal the value and copy it manually.");
    }
  };

  // An `aside`, so the panel stays the complementary landmark its title names;
  // the kit's Alert renders a div, so the warning tint is applied here.
  return (
    <aside
      className="grid gap-3 rounded-lg border border-warning/40 bg-warning/10 p-4 text-sm"
      aria-labelledby="one-time-title"
    >
      <div className="flex items-start gap-3">
        <KeyRound aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-warning" />
        <div className="grid gap-1">
          <strong
            id="one-time-title"
            ref={heading}
            tabIndex={-1}
            className="rounded-sm font-medium outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
          >
            Copy this {label} now. It will not be shown again.
          </strong>
          <p className="m-0 text-foreground/85">
            The value is hidden again when this tab moves to the background and removed after five
            minutes.
          </p>
        </div>
      </div>
      {revealed ? (
        <code className="block rounded-md border bg-card px-3 py-2 font-mono text-sm break-all select-all">
          {value}
        </code>
      ) : (
        <span className="block rounded-md border bg-card px-3 py-2 text-lg leading-6 tracking-[0.12em] text-muted-foreground select-none">
          <span aria-hidden="true">••••••••••••••••</span>
          <span className="sr-only">Secret value hidden</span>
        </span>
      )}
      <div className="flex flex-wrap gap-2">
        <Button variant="outline" size="sm" onClick={() => setRevealed((shown) => !shown)}>
          {revealed ? "Hide value" : "Reveal value"}
        </Button>
        <Button variant="outline" size="sm" onClick={() => void copy()}>
          Copy value
        </Button>
        <Button size="sm" onClick={onDismiss}>
          I have stored it securely
        </Button>
      </div>
      {copyStatus === "" ? null : (
        <p className="m-0 text-xs text-muted-foreground" aria-live="polite" aria-atomic="true">
          {copyStatus}
        </p>
      )}
    </aside>
  );
}
