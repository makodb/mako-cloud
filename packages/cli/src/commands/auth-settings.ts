import type {
  AuthProviderKind,
  AuthProviderUpdate,
  AuthSettingsUpdate,
  MagicLinkSettings,
} from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { Command, CommandArgs, OptionSpec } from "../cli/registry.js";
import { isJsonObject } from "./collections.js";
import { TENANT_OPTIONS, tenantFrom } from "./shared.js";

/** `^[a-z][a-z0-9-]{1,63}$`, as the API states it. */
const PROVIDER_NAME = /^[a-z][a-z0-9-]{1,63}$/u;
const MIN_LINK_TTL = 60;
const MAX_LINK_TTL = 3600;

const INPUT_OPTION: Readonly<Record<string, OptionSpec>> = {
  input: {
    type: "string",
    required: true,
    description:
      "The whole settings document {providers, redirectUrls, magicLinks}: @path, - for stdin, or inline JSON",
    placeholder: "<@file|-|json>",
  },
};

/**
 * Checks shape only, naming the field and never echoing a value, so a
 * mistake in the document cannot land a client secret in an error line.
 */
function kindFrom(value: unknown, at: string): AuthProviderKind {
  if (!isJsonObject(value)) throw usageError(`${at}.kind must be an object with a type`);
  if (value.type === "oidc") {
    if (typeof value.issuer !== "string" || value.issuer === "") {
      throw usageError(`${at}.kind.issuer must be the provider's issuer URL`);
    }
    return { type: "oidc", issuer: value.issuer };
  }
  if (value.type === "git_hub") return { type: "git_hub" };
  throw usageError(`${at}.kind.type must be oidc or git_hub`);
}

function providerFrom(value: unknown, index: number): AuthProviderUpdate {
  const at = `--input.providers[${index}]`;
  if (!isJsonObject(value)) {
    throw usageError(
      `${at} must be an object {name, kind, clientId, clientSecret?, scopes?, enabled}`,
    );
  }
  const { name, kind, clientId, clientSecret, scopes, enabled } = value;
  if (typeof name !== "string" || !PROVIDER_NAME.test(name)) {
    throw usageError(`${at}.name must be lowercase letters, digits, and hyphens (2-64 characters)`);
  }
  if (typeof clientId !== "string" || clientId === "") {
    throw usageError(`${at}.clientId must be a non-empty string`);
  }
  if (clientSecret !== undefined && (typeof clientSecret !== "string" || clientSecret === "")) {
    throw usageError(`${at}.clientSecret must be a non-empty string when given`);
  }
  if (scopes !== undefined && (!Array.isArray(scopes) || !scopes.every(isNonEmptyString))) {
    throw usageError(`${at}.scopes must be an array of non-empty strings`);
  }
  if (typeof enabled !== "boolean") throw usageError(`${at}.enabled must be true or false`);
  return {
    name,
    kind: kindFrom(kind, at),
    clientId,
    enabled,
    ...(clientSecret !== undefined ? { clientSecret } : {}),
    ...(scopes !== undefined ? { scopes: scopes as string[] } : {}),
  };
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value !== "";
}

function magicLinksFrom(value: unknown): MagicLinkSettings {
  const at = "--input.magicLinks";
  if (!isJsonObject(value)) throw usageError(`${at} must be an object {enabled, linkTtlSeconds}`);
  const { enabled, linkTtlSeconds } = value;
  if (typeof enabled !== "boolean") throw usageError(`${at}.enabled must be true or false`);
  if (
    typeof linkTtlSeconds !== "number" ||
    !Number.isInteger(linkTtlSeconds) ||
    linkTtlSeconds < MIN_LINK_TTL ||
    linkTtlSeconds > MAX_LINK_TTL
  ) {
    throw usageError(
      `${at}.linkTtlSeconds must be an integer between ${MIN_LINK_TTL} and ${MAX_LINK_TTL}`,
    );
  }
  return { enabled, linkTtlSeconds };
}

async function updateFrom(context: CommandContext, args: CommandArgs): Promise<AuthSettingsUpdate> {
  const value = await context.readJson(args.requireString("input"), "--input");
  if (!isJsonObject(value)) {
    throw usageError("--input must be a JSON object {providers, redirectUrls, magicLinks}");
  }
  const { providers, redirectUrls, magicLinks } = value;
  if (!Array.isArray(providers)) throw usageError("--input.providers must be an array");
  if (!Array.isArray(redirectUrls) || !redirectUrls.every(isNonEmptyString)) {
    throw usageError("--input.redirectUrls must be an array of absolute URLs");
  }
  return {
    providers: providers.map(providerFrom),
    redirectUrls: redirectUrls as string[],
    magicLinks: magicLinksFrom(magicLinks),
  };
}

async function getSettings(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.getAuthSettings(projectId, environmentId));
}

async function setSettings(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const request = await updateFrom(context, args);
  const client = await context.management();
  const settings = await client.updateAuthSettings(
    projectId,
    environmentId,
    request,
    context.idempotencyKey(),
  );
  // The answer says which providers hold a secret; the secrets themselves
  // went out once, in the request, and are never printed.
  context.out(settings);
}

export const authSettingsCommands: readonly Command[] = [
  {
    path: ["auth-settings", "get"],
    summary:
      "Show the environment's sign-in providers (without secrets), redirect allowlist, and magic-link settings",
    operations: ["getAuthSettings"],
    options: { ...TENANT_OPTIONS },
    run: getSettings,
  },
  {
    path: ["auth-settings", "set"],
    summary:
      "Replace the environment's sign-in settings from a JSON document; a provider without clientSecret keeps the installed one",
    operations: ["updateAuthSettings"],
    options: { ...TENANT_OPTIONS, ...INPUT_OPTION },
    run: setSettings,
  },
];
