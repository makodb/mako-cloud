import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { CommandArgs, OptionSpec } from "../cli/registry.js";

/** Options every environment-scoped command takes; env vars are the fallback. */
export const TENANT_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  project: {
    type: "string",
    short: "p",
    description: "Project id; also MAKO_PROJECT_ID",
    placeholder: "<project-id>",
  },
  env: {
    type: "string",
    short: "e",
    description: "Environment id; also MAKO_ENVIRONMENT_ID",
    placeholder: "<environment-id>",
  },
};

export const PROJECT_OPTION: Readonly<Record<string, OptionSpec>> = {
  project: TENANT_OPTIONS.project as OptionSpec,
};

export interface Tenant {
  readonly projectId: string;
  readonly environmentId: string;
}

export function projectFrom(context: CommandContext, args: CommandArgs): string {
  const projectId = args.string("project") ?? context.io.env.MAKO_PROJECT_ID;
  if (projectId === undefined || projectId === "") {
    throw usageError("--project <project-id> is required (or set MAKO_PROJECT_ID)");
  }
  return projectId;
}

export function tenantFrom(context: CommandContext, args: CommandArgs): Tenant {
  const projectId = projectFrom(context, args);
  const environmentId = args.string("env") ?? context.io.env.MAKO_ENVIRONMENT_ID;
  if (environmentId === undefined || environmentId === "") {
    throw usageError("--env <environment-id> is required (or set MAKO_ENVIRONMENT_ID)");
  }
  return { projectId, environmentId };
}

/** The option a secret-issuing command offers instead of printing the secret. */
export const SECRET_FILE_OPTION: Readonly<Record<string, OptionSpec>> = {
  "secret-file": {
    type: "string",
    description: "Write the secret to this file (created 0600) instead of printing it",
    placeholder: "<path>",
  },
};

const IN_FLIGHT_STATES: ReadonlySet<string> = new Set(["provisioning", "deleting"]);

/**
 * With --wait, polls a lifecycle resource until it leaves an in-flight state;
 * without it, returns the value as observed. The caller prints the result.
 */
export async function settleLifecycle<T extends { readonly id: string; readonly state: string }>(
  context: CommandContext,
  initial: T,
  poll: () => Promise<T>,
  label: string,
): Promise<T> {
  if (!context.globals.wait || !IN_FLIGHT_STATES.has(initial.state)) return initial;
  return context.waitFor(
    poll,
    (value) => !IN_FLIGHT_STATES.has(value.state),
    (value) => `${label} ${value.id}: ${value.state}`,
  );
}

/** Splits `key=value` pairs from repeated options into an object. */
export function pairsToObject(pairs: readonly string[], what: string): Record<string, string> {
  const result: Record<string, string> = {};
  for (const pair of pairs) {
    const separator = pair.indexOf("=");
    if (separator <= 0) throw usageError(`${what} must be key=value, got "${pair}"`);
    result[pair.slice(0, separator)] = pair.slice(separator + 1);
  }
  return result;
}
