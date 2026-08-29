import type { AllowedOrigins } from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { Command, CommandArgs, OptionSpec } from "../cli/registry.js";
import { TENANT_OPTIONS, tenantFrom } from "./shared.js";

/** An origin as a browser's `Origin` header carries it: scheme, host, an optional port, nothing after. */
const ORIGIN = /^(https?):\/\/([a-z0-9.-]+)(?::([0-9]{1,5}))?$/u;
const MAX_ORIGIN_LENGTH = 262;
const MAX_ORIGINS = 16;
const DEFAULT_PORTS: Readonly<Record<string, number>> = { https: 443, http: 80 };

/** Said on every write: the allowlist reaches the application API and nothing else. */
const SCOPE_NOTE =
  "the management and operator APIs never answer cross-origin, whatever is listed here";

const SET_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  ...TENANT_OPTIONS,
  origin: {
    type: "string",
    multiple: true,
    description:
      "Browser origin allowed to call this environment's application API, exactly as the browser sends it, e.g. https://app.example.com (repeatable, at most 16)",
    placeholder: "<url>",
  },
  none: {
    type: "boolean",
    description: "Allow no cross-origin access: clear the list",
  },
};

/**
 * One origin as the browser will send it: lowercase scheme and host, a port
 * only when it is not the scheme's default, and nothing after the host — a
 * path or a trailing slash would never match an `Origin` header, so it is
 * refused here rather than silently never matching. Plain http is admitted
 * only to loopback, where there is no certificate to speak of.
 */
function parseOrigin(raw: string): string {
  const origin = raw.trim().toLowerCase();
  const match = ORIGIN.exec(origin);
  if (match === null) {
    const detail = /^https?:\/\/[^/?#]*[/?#]/u.test(origin)
      ? "without a path, query, or trailing slash"
      : "as scheme://host[:port]";
    throw usageError(
      `--origin must be an exact browser origin such as https://app.example.com, ${detail}; got "${raw}"`,
    );
  }
  const scheme = match[1] ?? "";
  const host = match[2] ?? "";
  const port = match[3];
  if (port !== undefined) {
    const number = Number.parseInt(port, 10);
    if (String(number) !== port || number < 1 || number > 65535) {
      throw usageError(`--origin port must be 1-65535 without leading zeros; got "${raw}"`);
    }
    if (number === DEFAULT_PORTS[scheme]) {
      throw usageError(
        `--origin must omit the default port, a browser sends ${scheme}://${host}; got "${raw}"`,
      );
    }
  }
  if (scheme === "http" && !isLoopbackHost(host)) {
    throw usageError(
      `--origin must use https except to loopback (localhost or 127.0.0.1); got "${raw}"`,
    );
  }
  if (origin.length > MAX_ORIGIN_LENGTH) {
    throw usageError(`--origin must be at most ${MAX_ORIGIN_LENGTH} characters; got "${raw}"`);
  }
  return origin;
}

function isLoopbackHost(host: string): boolean {
  return host === "localhost" || /^127(?:\.[0-9]{1,3}){3}$/u.test(host);
}

/** The repeated --origin values, validated and de-duplicated in the order given. */
function originsFrom(args: CommandArgs): string[] {
  const origins = [...new Set(args.strings("origin").map(parseOrigin))];
  if (origins.length > MAX_ORIGINS) {
    throw usageError(
      `--origin may be given at most ${MAX_ORIGINS} times, got ${origins.length} distinct origins`,
    );
  }
  return origins;
}

function describeOrigins(environmentId: string, allowed: AllowedOrigins): string {
  const count = allowed.allowedOrigins.length;
  const what =
    count === 0
      ? "no cross-origin access"
      : `cross-origin calls from ${count} origin${count === 1 ? "" : "s"} to its application API`;
  return `environment ${environmentId} allows ${what}; ${SCOPE_NOTE}`;
}

/** The list itself is the output: one origin per line, `(none)` when empty. */
function printOrigins(
  context: CommandContext,
  environmentId: string,
  allowed: AllowedOrigins,
): void {
  if (context.json) {
    context.out(allowed);
    return;
  }
  context.info(describeOrigins(environmentId, allowed));
  context.out(allowed.allowedOrigins, { line: (origin) => String(origin) });
}

async function getOrigins(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  printOrigins(context, environmentId, await client.getAllowedOrigins(projectId, environmentId));
}

async function setOrigins(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const none = args.boolean("none");
  if (none && args.strings("origin").length > 0) {
    throw usageError("--none and --origin are mutually exclusive");
  }
  if (!none && args.strings("origin").length === 0) {
    throw usageError("--origin <url> (repeatable) or --none is required");
  }
  const allowedOrigins = none ? [] : originsFrom(args);
  const client = await context.management();
  const allowed = await client.updateAllowedOrigins(
    projectId,
    environmentId,
    { allowedOrigins },
    context.idempotencyKey(),
  );
  printOrigins(context, environmentId, allowed);
}

export const allowedOriginsCommands: readonly Command[] = [
  {
    path: ["allowed-origins", "get"],
    summary: "Show the browser origins allowed to call this environment's application API",
    operations: ["getAllowedOrigins"],
    options: { ...TENANT_OPTIONS },
    run: getOrigins,
  },
  {
    path: ["allowed-origins", "set"],
    summary:
      "Replace the browser origins allowed to call this environment's application API; --none allows no cross-origin access",
    operations: ["updateAllowedOrigins"],
    options: SET_OPTIONS,
    run: setOrigins,
  },
];
