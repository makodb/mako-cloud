import { useCallback, useEffect, useState } from "react";

import type { AllowedOrigins } from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";

/** An origin as a browser's `Origin` header carries it: scheme, host, an optional port, nothing after. */
const ORIGIN = /^(https?):\/\/([a-z0-9.-]+)(?::([0-9]{1,5}))?$/u;
const MAX_ORIGIN_LENGTH = 262;
const MAX_ORIGINS = 16;
const DEFAULT_PORTS: Readonly<Record<string, number>> = { https: 443, http: 80 };

// The browser origins allowed to call this environment's application API.
// The project's ordinary API URL is on the platform hostname, so a browser
// application needs its origin listed here whatever host it is served from;
// a custom domain is a convenience, never a prerequisite. The list is edited
// whole — what is saved replaces what was there — and checked here first, so
// a path or a trailing slash is refused with its reason instead of being
// saved and never matching a browser.
export function AllowedOriginsSection({
  projectId,
  environmentId,
}: {
  readonly projectId: string;
  readonly environmentId: string;
}) {
  const client = useManagementClient();
  const [allowed, setAllowed] = useState<AllowedOrigins | null>(null);
  const [draft, setDraft] = useState("");
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    try {
      const current = await client.getAllowedOrigins(projectId, environmentId);
      setAllowed(current);
      setDraft(current.allowedOrigins.join("\n"));
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  }, [client, environmentId, projectId]);

  useEffect(() => {
    void load();
  }, [load]);

  const save = async () => {
    setStatus(null);
    let allowedOrigins: string[];
    try {
      allowedOrigins = parseOrigins(draft);
    } catch (error) {
      setFailure(failureFrom(error));
      return;
    }
    setSaving(true);
    try {
      const updated = await client.updateAllowedOrigins(
        projectId,
        environmentId,
        { allowedOrigins },
        idempotencyKey(),
      );
      setAllowed(updated);
      setDraft(updated.allowedOrigins.join("\n"));
      setFailure(null);
      setStatus(describeOrigins(updated));
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="allowed-origins" aria-labelledby="allowed-origins-heading">
      <h2 id="allowed-origins-heading">Allowed origins</h2>
      <p>
        A browser application must list its own origin here before it can call this environment's
        application API — authentication, documents, replication, storage, and function invocation —
        from a page served somewhere else. This holds on the project's own API URL as well as on any
        custom domain serving the environment. The management and operator APIs never answer
        cross-origin calls, whatever is listed here.
      </p>
      <ApiFailureNotice failure={failure} />
      {status === null ? null : (
        <p className="notice success" role="status">
          {status}
        </p>
      )}
      {allowed === null ? (
        <p>Loading…</p>
      ) : (
        <>
          <h3>Allowed now</h3>
          <OriginList origins={allowed.allowedOrigins} />
          <form
            className="allowed-origins-editor"
            aria-label="Allowed origins"
            onSubmit={(event) => {
              event.preventDefault();
              void save();
            }}
          >
            <label>
              Origins, one per line
              <textarea
                name="allowedOrigins"
                rows={5}
                value={draft}
                onChange={(event) => setDraft(event.currentTarget.value)}
                placeholder="https://app.example.com"
                spellCheck={false}
              />
            </label>
            <p className="allowed-origins-note">
              An origin is matched exactly as the browser sends it — scheme, host, and port, with no
              path or trailing slash: <code>https://app.example.com</code>, or{" "}
              <code>http://127.0.0.1:5173</code> for a local development server (plain http only to
              loopback). At most {MAX_ORIGINS}; an empty list allows no cross-origin access. Saving
              replaces the whole list.
            </p>
            <div className="button-row">
              <button type="submit" disabled={saving}>
                {saving ? "Saving…" : "Save origins"}
              </button>
            </div>
          </form>
        </>
      )}
    </section>
  );
}

/** The allowlist as the browser will match it; an empty one is said outright. */
function OriginList({ origins }: { readonly origins: readonly string[] }) {
  if (origins.length === 0) {
    return (
      <p className="allowed-origins-none">
        None — no browser on another origin can call this environment's API.
      </p>
    );
  }
  return (
    <ul className="allowed-origins-list">
      {origins.map((origin) => (
        <li key={origin}>
          <code>{origin}</code>
        </li>
      ))}
    </ul>
  );
}

function describeOrigins(allowed: AllowedOrigins): string {
  const count = allowed.allowedOrigins.length;
  return count === 0
    ? "This environment now allows no cross-origin access."
    : `This environment now allows cross-origin calls from ${count} origin${count === 1 ? "" : "s"}.`;
}

/**
 * One origin as the browser will send it: lowercase scheme and host, a port
 * only when it is not the scheme's default, and nothing after the host — a
 * path or a trailing slash would never match an `Origin` header. Plain http
 * is admitted only to loopback, where there is no certificate to speak of.
 */
function parseOrigin(raw: string): string {
  const origin = raw.toLowerCase();
  const match = ORIGIN.exec(origin);
  if (match === null) {
    throw new FormInputError(
      `"${raw}" is not an exact browser origin. Use scheme://host[:port] such as https://app.example.com, with no path, query, or trailing slash.`,
    );
  }
  const scheme = match[1] ?? "";
  const host = match[2] ?? "";
  const port = match[3];
  if (port !== undefined) {
    const number = Number.parseInt(port, 10);
    if (String(number) !== port || number < 1 || number > 65535) {
      throw new FormInputError(`"${raw}" has an invalid port; use 1-65535 without leading zeros.`);
    }
    if (number === DEFAULT_PORTS[scheme]) {
      throw new FormInputError(
        `"${raw}" names the default port; a browser sends ${scheme}://${host}.`,
      );
    }
  }
  if (scheme === "http" && !isLoopbackHost(host)) {
    throw new FormInputError(
      `"${raw}" uses plain http, which is allowed only to loopback (localhost or 127.0.0.1); use https.`,
    );
  }
  if (origin.length > MAX_ORIGIN_LENGTH) {
    throw new FormInputError(`"${raw}" is longer than ${MAX_ORIGIN_LENGTH} characters.`);
  }
  return origin;
}

function isLoopbackHost(host: string): boolean {
  return host === "localhost" || /^127(?:\.[0-9]{1,3}){3}$/u.test(host);
}

/** One origin per line (a comma separates too), validated and de-duplicated in order. */
function parseOrigins(text: string): string[] {
  const origins = [
    ...new Set(
      text
        .split(/[\n,]/u)
        .map((line) => line.trim())
        .filter((line) => line !== "")
        .map(parseOrigin),
    ),
  ];
  if (origins.length > MAX_ORIGINS) {
    throw new FormInputError(
      `At most ${MAX_ORIGINS} origins can be allowed; ${origins.length} were given.`,
    );
  }
  return origins;
}

class FormInputError extends Error {}

function failureFrom(error: unknown): ConsoleApiFailure {
  return error instanceof FormInputError
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}
