import type { CustomDomain, CustomDomainCreate } from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec, PositionalSpec } from "../cli/registry.js";
import { PROJECT_OPTION, TENANT_OPTIONS, projectFrom, tenantFrom } from "./shared.js";

/** A lowercase fully qualified name: labels of letters, digits, and inner hyphens, two or more. */
const HOSTNAME =
  /^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/u;
const MAX_HOSTNAME_LENGTH = 253;

const DOMAIN_COLUMNS: readonly TableColumn[] = [
  { key: "id" },
  { key: "hostname" },
  { key: "environmentId" },
  { key: "state" },
  { key: "lastError", label: "error" },
  { key: "verifiedAt" },
  { key: "lastCheckedAt" },
];

const DOMAIN_ID: PositionalSpec = {
  name: "domain-id",
  description: "Custom domain id (dom_…)",
  required: true,
};

/**
 * Domains belong to the project; `--env` here names the environment whose
 * API and functions the name serves, not the scope of the command.
 */
const ADD_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  ...PROJECT_OPTION,
  hostname: {
    type: "string",
    description: "The fully qualified DNS name to serve, e.g. api.example.com",
    placeholder: "<hostname>",
  },
  env: {
    ...(TENANT_OPTIONS.env as OptionSpec),
    description: "Environment the domain serves (its API and functions); also MAKO_ENVIRONMENT_ID",
  },
};

function domainOf(args: CommandArgs): string {
  return args.requirePositional(0, "domain-id");
}

function hostnameFrom(args: CommandArgs): string {
  const raw = args.string("hostname");
  if (raw === undefined) throw usageError("--hostname <hostname> is required");
  const hostname = raw.trim().toLowerCase().replace(/\.$/u, "");
  if (hostname.length > MAX_HOSTNAME_LENGTH || !HOSTNAME.test(hostname)) {
    throw usageError(
      `--hostname must be a fully qualified DNS name such as api.example.com, got "${raw}"`,
    );
  }
  return hostname;
}

/** The line a script or a person acts on: what the check found. */
function describeOutcome(domain: CustomDomain): string {
  if (domain.state === "verified") {
    return `domain ${domain.id} (${domain.hostname}) is verified and served`;
  }
  const reason = domain.lastError === null ? "" : `: ${domain.lastError}`;
  const consequence = domain.state === "failed" ? "; serving stopped until it verifies again" : "";
  return `domain ${domain.id} (${domain.hostname}) is ${domain.state}${reason}${consequence}`;
}

/**
 * Prints the domain, then the TXT record to publish as its own block so it
 * is not lost among the fields. With --json the record is in the response.
 */
function printWithRecord(context: CommandContext, domain: CustomDomain): void {
  if (context.json) {
    context.out(domain);
    return;
  }
  const { verification, ...fields } = domain;
  context.out(fields);
  const rows: readonly (readonly [string, string])[] = [
    ["name", verification.recordName],
    ["type", verification.recordType],
    ["value", verification.recordValue],
  ];
  const rule = "-".repeat(Math.max(40, ...rows.map(([, value]) => value.length + 7)));
  context.io.stdout.write(
    [
      rule,
      `DNS record to publish for ${domain.hostname}`,
      ...rows.map(([label, value]) => `${label.padEnd(5)}  ${value}`),
      rule,
    ]
      .map((line) => `${line}\n`)
      .join(""),
  );
}

async function listDomains(context: CommandContext, args: CommandArgs): Promise<void> {
  const projectId = projectFrom(context, args);
  const client = await context.management();
  context.out(await client.listCustomDomains(projectId), { columns: DOMAIN_COLUMNS });
}

async function getDomain(context: CommandContext, args: CommandArgs): Promise<void> {
  const domainId = domainOf(args);
  const projectId = projectFrom(context, args);
  const client = await context.management();
  context.out(await client.getCustomDomain(projectId, domainId));
}

async function addDomain(context: CommandContext, args: CommandArgs): Promise<void> {
  const hostname = hostnameFrom(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const request: CustomDomainCreate = { hostname, environmentId };
  const client = await context.management();
  const domain = await client.createCustomDomain(projectId, request, context.idempotencyKey());
  if (!context.json) {
    context.info(
      `domain ${domain.id} is ${domain.state}; publish the DNS record below, then run: ` +
        `mako domains verify ${domain.id} --project ${projectId}`,
    );
  }
  printWithRecord(context, domain);
}

async function verifyDomain(context: CommandContext, args: CommandArgs): Promise<void> {
  const domainId = domainOf(args);
  const projectId = projectFrom(context, args);
  const client = await context.management();
  const domain = await client.verifyCustomDomain(projectId, domainId, context.idempotencyKey());
  if (!context.json) context.info(describeOutcome(domain));
  context.out(domain);
}

async function removeDomain(context: CommandContext, args: CommandArgs): Promise<void> {
  const domainId = domainOf(args);
  const projectId = projectFrom(context, args);
  const client = await context.management();
  await client.deleteCustomDomain(projectId, domainId, context.idempotencyKey());
  if (context.json) context.out({ id: domainId, state: "removed" });
  else context.info(`domain ${domainId} removed; nothing is served on its name any more`);
}

export const domainsCommands: readonly Command[] = [
  {
    path: ["domains", "list"],
    summary:
      "List the project's custom domains with the environment each serves, its state, and the last check",
    operations: ["listCustomDomains"],
    options: { ...PROJECT_OPTION },
    run: listDomains,
  },
  {
    path: ["domains", "get"],
    summary: "Show a custom domain, its DNS verification record, and why the last check failed",
    operations: ["getCustomDomain"],
    positionals: [DOMAIN_ID],
    options: { ...PROJECT_OPTION },
    run: getDomain,
  },
  {
    path: ["domains", "add"],
    summary:
      "Add a custom domain that serves one environment's API and functions; prints the TXT record to publish",
    operations: ["createCustomDomain"],
    options: ADD_OPTIONS,
    run: addDomain,
  },
  {
    path: ["domains", "verify"],
    summary:
      "Check a domain's DNS record now instead of at the next periodic check and show the outcome",
    operations: ["verifyCustomDomain"],
    positionals: [DOMAIN_ID],
    options: { ...PROJECT_OPTION },
    run: verifyDomain,
  },
  {
    path: ["domains", "remove"],
    summary:
      "Remove a custom domain; serving on its name stops and its certificate is no longer renewed",
    operations: ["deleteCustomDomain"],
    positionals: [DOMAIN_ID],
    options: { ...PROJECT_OPTION },
    destructive: { action: "remove domain", resource: domainOf },
    run: removeDomain,
  },
];
