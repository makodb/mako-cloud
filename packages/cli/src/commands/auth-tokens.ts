import type {
  AutomationPermission,
  AutomationScope,
  AutomationToken,
} from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec } from "../cli/registry.js";
import { SECRET_FILE_OPTION } from "./shared.js";

/**
 * Every permission an automation token can carry. Typed as a complete record so
 * the build fails when the API grows a permission this list does not name.
 */
const PERMISSION_SET: Readonly<Record<AutomationPermission, true>> = {
  organization_read: true,
  project_read: true,
  project_write: true,
  environment_read: true,
  environment_write: true,
  collection_write: true,
  policy_write: true,
  function_deploy: true,
  audit_read: true,
};

export const AUTOMATION_PERMISSIONS = Object.keys(
  PERMISSION_SET,
) as readonly AutomationPermission[];

function isPermission(value: string): value is AutomationPermission {
  return Object.hasOwn(PERMISSION_SET, value);
}

const AUTOMATION_TOKEN_ID = /^atm_[A-Za-z0-9_-]{8,64}$/u;

const DURATION = /^(\d+)\s*([mhdw])$/u;
const UNIT_MS: Readonly<Record<string, number>> = {
  m: 60_000,
  h: 3_600_000,
  d: 86_400_000,
  w: 7 * 86_400_000,
};

/** Parses `30d`, `12h`, `45m`, or `2w` into milliseconds. */
export function durationMs(value: string, option: string): number {
  const match = DURATION.exec(value.trim());
  const amount = match?.[1] === undefined ? 0 : Number.parseInt(match[1], 10);
  const unit = match?.[2] === undefined ? undefined : UNIT_MS[match[2]];
  if (unit === undefined || amount <= 0) {
    throw usageError(`--${option} must be a duration like 30d, 12h, or 45m (got "${value}")`);
  }
  return amount * unit;
}

/** The ISO instant `--<option>` from now, for API fields that take an `expiresAt`. */
export function expiresAtFrom(args: CommandArgs, option = "expires-in", fallback?: string): string {
  const value = args.string(option) ?? fallback;
  if (value === undefined || value === "") throw usageError(`--${option} is required`);
  return new Date(Date.now() + durationMs(value, option)).toISOString();
}

const TEAM_OPTION: Readonly<Record<string, OptionSpec>> = {
  team: {
    type: "string",
    description: "Team the token belongs to",
    placeholder: "<team-id>",
    required: true,
  },
};

const EXPIRES_IN_OPTION: Readonly<Record<string, OptionSpec>> = {
  "expires-in": {
    type: "string",
    description: "Lifetime from now, like 30d, 12h, or 45m",
    placeholder: "<duration>",
    required: true,
  },
};

function permissionsFrom(args: CommandArgs): AutomationPermission[] {
  const values = args
    .strings("permission")
    .flatMap((value) => value.split(","))
    .map((value) => value.trim())
    .filter((value) => value !== "");
  if (values.length === 0) {
    throw usageError(`--permission is required; one of: ${AUTOMATION_PERMISSIONS.join(", ")}`);
  }
  const permissions: AutomationPermission[] = [];
  for (const value of values) {
    if (!isPermission(value)) {
      throw usageError(
        `unknown permission "${value}"; valid permissions: ${AUTOMATION_PERMISSIONS.join(", ")}`,
      );
    }
    if (!permissions.includes(value)) permissions.push(value);
  }
  return permissions;
}

function scopeFrom(args: CommandArgs): AutomationScope {
  const permissions = permissionsFrom(args);
  const projectId = args.string("project");
  const environmentId = args.string("env");
  if (environmentId !== undefined && projectId === undefined) {
    throw usageError("--env needs --project: an environment scope names its project");
  }
  return {
    permissions,
    ...(projectId !== undefined ? { projectId } : {}),
    ...(environmentId !== undefined ? { environmentId } : {}),
  };
}

/** The flat view humans see; JSON output keeps the API's nested scope. */
function tokenRow(token: AutomationToken): Record<string, unknown> {
  return {
    id: token.id,
    name: token.name,
    status: token.status,
    permissions: token.scope.permissions.join(","),
    projectId: token.scope.projectId ?? null,
    environmentId: token.scope.environmentId ?? null,
    expiresAt: token.expiresAt,
    createdAt: token.createdAt,
    ...(token.revokedAt !== undefined ? { revokedAt: token.revokedAt } : {}),
  };
}

const TOKEN_COLUMNS: readonly TableColumn[] = [
  { key: "id" },
  { key: "name" },
  { key: "status" },
  { key: "permissions" },
  { key: "projectId" },
  { key: "environmentId" },
  { key: "expiresAt" },
];

async function issueToken(
  context: CommandContext,
  issue: { readonly token: AutomationToken; readonly secret: string },
  secretFile: string | undefined,
): Promise<void> {
  await context.secret(
    `automation token ${issue.token.name}`,
    issue.secret,
    context.json ? { token: issue.token } : tokenRow(issue.token),
    secretFile,
  );
}

export const authTokensCommands: readonly Command[] = [
  {
    path: ["auth", "token", "create"],
    summary: "Issue an automation token for a team; its secret is shown once",
    operations: ["createAutomationToken"],
    options: {
      ...TEAM_OPTION,
      name: {
        type: "string",
        description: "A name that tells this token apart in the list",
        placeholder: "<name>",
        required: true,
      },
      permission: {
        type: "string",
        multiple: true,
        description: `Permission to grant (repeatable or comma-separated): ${AUTOMATION_PERMISSIONS.join(", ")}`,
        placeholder: "<permission>",
        required: true,
      },
      project: {
        type: "string",
        short: "p",
        description: "Narrow the token to one project",
        placeholder: "<project-id>",
      },
      env: {
        type: "string",
        short: "e",
        description: "Narrow the token to one environment (needs --project)",
        placeholder: "<environment-id>",
      },
      ...EXPIRES_IN_OPTION,
      ...SECRET_FILE_OPTION,
    },
    run: async (context, args) => {
      const teamId = args.requireString("team");
      const input = {
        name: args.requireString("name"),
        scope: scopeFrom(args),
        expiresAt: expiresAtFrom(args),
      };
      const client = await context.management();
      await issueToken(
        context,
        await client.createAutomationToken(teamId, input),
        args.string("secret-file"),
      );
    },
  },
  {
    path: ["auth", "token", "list"],
    summary: "List a team's automation tokens",
    operations: ["listAutomationTokens"],
    options: TEAM_OPTION,
    run: async (context, args) => {
      const client = await context.management();
      const tokens = await client.listAutomationTokens(args.requireString("team"));
      context.out(context.json ? tokens : tokens.map(tokenRow), { columns: TOKEN_COLUMNS });
    },
  },
  {
    path: ["auth", "token", "revoke"],
    summary: "Revoke an automation token; automation using it loses access immediately",
    operations: ["revokeAutomationToken"],
    positionals: [{ name: "token-id", description: "Automation token id", required: true }],
    options: TEAM_OPTION,
    destructive: {
      action: "revoke automation token",
      resource: (args) => args.requirePositional(0, "token-id"),
    },
    run: async (context, args) => {
      const tokenId = args.requirePositional(0, "token-id");
      const teamId = args.requireString("team");
      const client = await context.management();
      await client.revokeAutomationToken(teamId, tokenId);
      if (context.json) context.out({ id: tokenId, teamId, status: "revoked" });
      else context.info(`Revoked automation token ${tokenId}.`);
    },
  },
  {
    path: ["auth", "token", "rotate"],
    summary: "Replace an automation token with a new one; the old token stops working",
    operations: ["rotateAutomationToken"],
    positionals: [
      { name: "token-id", description: "Automation token id to replace", required: true },
    ],
    options: {
      ...TEAM_OPTION,
      "replacement-id": {
        type: "string",
        description: "Id for the replacement token (atm_ followed by 8-64 characters)",
        placeholder: "<token-id>",
        required: true,
      },
      ...EXPIRES_IN_OPTION,
      ...SECRET_FILE_OPTION,
    },
    destructive: {
      action: "rotate automation token",
      resource: (args) => args.requirePositional(0, "token-id"),
    },
    run: async (context, args) => {
      const tokenId = args.requirePositional(0, "token-id");
      const teamId = args.requireString("team");
      const replacementId = args.requireString("replacement-id");
      if (!AUTOMATION_TOKEN_ID.test(replacementId)) {
        throw usageError(
          "--replacement-id must be atm_ followed by 8 to 64 letters, digits, _ or -",
        );
      }
      const expiresAt = expiresAtFrom(args);
      const client = await context.management();
      const issue = await client.rotateAutomationToken(
        teamId,
        tokenId,
        replacementId,
        expiresAt,
        context.idempotencyKey(),
      );
      await issueToken(context, issue, args.string("secret-file"));
    },
  },
];
