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

  return (
    <aside className="one-time-value" aria-labelledby="one-time-title">
      <strong id="one-time-title" ref={heading} tabIndex={-1}>
        Copy this {label} now. It will not be shown again.
      </strong>
      <p>
        The value is hidden again when this tab moves to the background and removed after five
        minutes.
      </p>
      {revealed ? (
        <code className="secret-value">{value}</code>
      ) : (
        <span className="secret-placeholder">
          <span aria-hidden="true">••••••••••••••••</span>
          <span className="visually-hidden">Secret value hidden</span>
        </span>
      )}
      <div className="button-row">
        <button type="button" className="secondary" onClick={() => setRevealed((shown) => !shown)}>
          {revealed ? "Hide value" : "Reveal value"}
        </button>
        <button type="button" className="secondary" onClick={() => void copy()}>
          Copy value
        </button>
        <button type="button" onClick={onDismiss}>
          I have stored it securely
        </button>
      </div>
      {copyStatus === "" ? null : (
        <p className="copy-status" aria-live="polite" aria-atomic="true">
          {copyStatus}
        </p>
      )}
    </aside>
  );
}
