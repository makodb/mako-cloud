import { MoonIcon, SunIcon } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { Button, type ButtonProps } from "./components/button.js";
import { THEME_ATTRIBUTE } from "./tokens.js";

/** What the person asked for; `system` means the device decides. */
export type ThemePreference = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

function readPreference(storageKey: string): ThemePreference {
  try {
    const stored = window.localStorage.getItem(storageKey);
    return stored === "light" || stored === "dark" ? stored : "system";
  } catch {
    return "system";
  }
}

function deviceTheme(): ResolvedTheme {
  return typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

/**
 * The theme is a per-device preference, kept under `storageKey` and applied
 * to the document root as `data-theme`, which the stylesheet's tokens follow.
 * No preference means no attribute, and the device's own setting shows
 * through.
 */
export function useTheme(storageKey: string): {
  preference: ThemePreference;
  resolved: ResolvedTheme;
  setPreference: (preference: ThemePreference) => void;
  toggle: () => void;
} {
  const [preference, setPreference] = useState<ThemePreference>(() => readPreference(storageKey));
  const [device, setDevice] = useState<ResolvedTheme>(deviceTheme);

  useEffect(() => {
    const root = document.documentElement;
    if (preference === "system") {
      root.removeAttribute(THEME_ATTRIBUTE);
    } else {
      root.setAttribute(THEME_ATTRIBUTE, preference);
    }
    try {
      if (preference === "system") window.localStorage.removeItem(storageKey);
      else window.localStorage.setItem(storageKey, preference);
    } catch {
      // Not remembering is fine.
    }
  }, [preference, storageKey]);

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => setDevice(media.matches ? "dark" : "light");
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, []);

  const resolved = preference === "system" ? device : preference;
  const toggle = useCallback(() => {
    setPreference(resolved === "dark" ? "light" : "dark");
  }, [resolved]);
  return { preference, resolved, setPreference, toggle };
}

/** A button that flips between light and dark, named for what it will do and pressed while dark. */
export function ThemeToggle({
  resolved,
  onToggle,
  className,
  ...props
}: Omit<ButtonProps, "onClick" | "children"> & {
  resolved: ResolvedTheme;
  onToggle: () => void;
}) {
  // A toggle's name says what it controls and its pressed state says where it
  // stands; a name that changes with the state says both and agrees with
  // neither. "Theme", not "mode": an application screen may have a control
  // labelled "Mode", and a label is matched by substring, so a test looking
  // for that one must not find this button instead.
  return (
    <Button
      variant="ghost"
      size="icon"
      aria-label="Dark theme"
      title={resolved === "dark" ? "Switch to the light theme" : "Switch to the dark theme"}
      aria-pressed={resolved === "dark"}
      onClick={onToggle}
      className={className}
      {...props}
    >
      {resolved === "dark" ? <SunIcon /> : <MoonIcon />}
    </Button>
  );
}
