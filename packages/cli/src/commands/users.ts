import type {
  AdminCreateUserRequest,
  AdminUpdateUserMetadataRequest,
} from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec, PositionalSpec } from "../cli/registry.js";
import { isJsonObject } from "./collections.js";
import { TENANT_OPTIONS, tenantFrom } from "./shared.js";

type JsonObject = Record<string, unknown>;

const USER_COLUMNS: readonly TableColumn[] = [
  { key: "id" },
  { key: "email" },
  { key: "status" },
  { key: "createdAt" },
];

const USER_ID: PositionalSpec = {
  name: "user-id",
  description: "Application user id",
  required: true,
};

const SESSION_ID: PositionalSpec = {
  name: "session-id",
  description: "Session id (see `mako users get`)",
  required: true,
};

/** Options `create` and `invite` share: the address and both metadata objects. */
const NEW_USER_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  email: {
    type: "string",
    required: true,
    description: "Email address of the user",
    placeholder: "<email>",
  },
  "trusted-metadata": {
    type: "string",
    description:
      "Trusted metadata JSON, set by you and read by policies: @path, - for stdin, or inline (default: {})",
    placeholder: "<@file|-|json>",
  },
  "profile-metadata": {
    type: "string",
    description:
      "Profile metadata JSON the user may edit: @path, - for stdin, or inline (default: {})",
    placeholder: "<@file|-|json>",
  },
};

function userOf(args: CommandArgs): string {
  return args.requirePositional(0, "user-id");
}

async function metadataFrom(
  context: CommandContext,
  args: CommandArgs,
  name: string,
): Promise<JsonObject> {
  const source = args.string(name);
  if (source === undefined) return {};
  const value = await context.readJson(source, `--${name}`);
  if (!isJsonObject(value)) throw usageError(`--${name} must be a JSON object`);
  return value;
}

async function newUserFrom(
  context: CommandContext,
  args: CommandArgs,
): Promise<AdminCreateUserRequest> {
  const email = args.requireString("email");
  if (!email.includes("@")) throw usageError("--email must be an email address");
  return {
    email,
    trustedMetadata: await metadataFrom(context, args, "trusted-metadata"),
    profileMetadata: await metadataFrom(context, args, "profile-metadata"),
  };
}

async function search(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const query = args.string("query");
  const limit = args.integer("limit");
  if (limit !== undefined && (limit < 1 || limit > 100)) {
    throw usageError("--limit must be between 1 and 100");
  }
  const client = await context.management();
  const result = await client.searchApplicationUsers(projectId, environmentId, {
    ...(query !== undefined && query !== "" ? { query } : {}),
    ...(limit !== undefined ? { limit } : {}),
  });
  if (context.json) {
    context.out(result);
    return;
  }
  context.out(result.users, { columns: USER_COLUMNS });
  if (result.truncated) {
    context.info("more users match than were returned; narrow --query or raise --limit (max 100)");
  }
}

async function create(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const request = await newUserFrom(context, args);
  const client = await context.management();
  context.out(
    await client.createApplicationUser(projectId, environmentId, request, context.idempotencyKey()),
  );
}

async function invite(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const request = await newUserFrom(context, args);
  const client = await context.management();
  context.out(
    await client.inviteApplicationUser(projectId, environmentId, request, context.idempotencyKey()),
  );
}

async function get(context: CommandContext, args: CommandArgs): Promise<void> {
  const userId = userOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.getApplicationUser(projectId, environmentId, userId));
}

async function updateMetadata(context: CommandContext, args: CommandArgs): Promise<void> {
  const userId = userOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const input = await context.readJson(args.requireString("input"), "--input");
  if (!isJsonObject(input)) {
    throw usageError("--input must be a JSON object {trustedMetadata, profileMetadata}");
  }
  const { trustedMetadata, profileMetadata } = input;
  if (!isJsonObject(trustedMetadata) || !isJsonObject(profileMetadata)) {
    throw usageError(
      "--input must carry both trustedMetadata and profileMetadata objects; the API replaces both",
    );
  }
  const request: AdminUpdateUserMetadataRequest = { trustedMetadata, profileMetadata };
  const client = await context.management();
  context.out(
    await client.updateApplicationUserMetadata(projectId, environmentId, userId, request),
  );
}

function userAction(
  call: (
    client: Awaited<ReturnType<CommandContext["management"]>>,
    projectId: string,
    environmentId: string,
    userId: string,
  ) => Promise<unknown>,
): Command["run"] {
  return async (context, args) => {
    const userId = userOf(args);
    const { projectId, environmentId } = tenantFrom(context, args);
    const client = await context.management();
    context.out(await call(client, projectId, environmentId, userId));
  };
}

async function revokeSession(context: CommandContext, args: CommandArgs): Promise<void> {
  const userId = userOf(args);
  const sessionId = args.requirePositional(1, "session-id");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(
    await client.revokeApplicationUserSession(projectId, environmentId, userId, sessionId),
  );
}

export const usersCommands: readonly Command[] = [
  {
    path: ["users", "search"],
    summary: "Search application users by id or email (bounded; no credential material)",
    operations: ["searchApplicationUsers"],
    options: {
      ...TENANT_OPTIONS,
      query: {
        type: "string",
        short: "q",
        description: "Match ids and email addresses containing this text",
        placeholder: "<text>",
      },
      limit: {
        type: "string",
        description: "Maximum users to return, 1-100 (default: 50)",
        placeholder: "<n>",
      },
    },
    run: search,
  },
  {
    path: ["users", "create"],
    summary: "Create an application user directly, without an invitation",
    operations: ["createApplicationUser"],
    options: { ...TENANT_OPTIONS, ...NEW_USER_OPTIONS },
    run: create,
  },
  {
    path: ["users", "invite"],
    summary: "Invite an application user; they finish signing up themselves",
    operations: ["inviteApplicationUser"],
    options: { ...TENANT_OPTIONS, ...NEW_USER_OPTIONS },
    run: invite,
  },
  {
    path: ["users", "get"],
    summary: "Show an application user, their metadata, and their sessions",
    operations: ["getApplicationUser"],
    positionals: [USER_ID],
    options: { ...TENANT_OPTIONS },
    run: get,
  },
  {
    path: ["users", "update-metadata"],
    summary: "Replace a user's trusted and profile metadata",
    operations: ["updateApplicationUserMetadata"],
    positionals: [USER_ID],
    options: {
      ...TENANT_OPTIONS,
      input: {
        type: "string",
        required: true,
        description:
          "Metadata JSON {trustedMetadata, profileMetadata}: @path, - for stdin, or inline",
        placeholder: "<@file|-|json>",
      },
    },
    run: updateMetadata,
  },
  {
    path: ["users", "disable"],
    summary: "Disable a user; their sessions stop working until restored",
    operations: ["disableApplicationUser"],
    positionals: [USER_ID],
    options: { ...TENANT_OPTIONS },
    destructive: { action: "disable user", resource: userOf },
    run: userAction((client, projectId, environmentId, userId) =>
      client.disableApplicationUser(projectId, environmentId, userId),
    ),
  },
  {
    path: ["users", "restore"],
    summary: "Restore a disabled user",
    operations: ["restoreApplicationUser"],
    positionals: [USER_ID],
    options: { ...TENANT_OPTIONS },
    run: userAction((client, projectId, environmentId, userId) =>
      client.restoreApplicationUser(projectId, environmentId, userId),
    ),
  },
  {
    path: ["users", "delete"],
    summary: "Delete a user",
    operations: ["deleteApplicationUser"],
    positionals: [USER_ID],
    options: { ...TENANT_OPTIONS },
    destructive: { action: "delete user", resource: userOf },
    run: userAction((client, projectId, environmentId, userId) =>
      client.deleteApplicationUser(projectId, environmentId, userId),
    ),
  },
  {
    path: ["users", "revoke-sessions"],
    summary: "Revoke every session of a user",
    operations: ["revokeApplicationUserSessions"],
    positionals: [USER_ID],
    options: { ...TENANT_OPTIONS },
    destructive: { action: "revoke every session of user", resource: userOf },
    run: userAction((client, projectId, environmentId, userId) =>
      client.revokeApplicationUserSessions(projectId, environmentId, userId),
    ),
  },
  {
    path: ["users", "revoke-session"],
    summary: "Revoke one session of a user",
    operations: ["revokeApplicationUserSession"],
    positionals: [USER_ID, SESSION_ID],
    options: { ...TENANT_OPTIONS },
    destructive: {
      action: "revoke session",
      resource: (args) => args.requirePositional(1, "session-id"),
    },
    run: revokeSession,
  },
];
