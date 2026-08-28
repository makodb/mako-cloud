import { ManagementApiError } from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import type { StoredSession } from "../cli/credentials.js";
import { authError, CliError, EXIT, usageError } from "../cli/errors.js";
import { promptLine, promptSecret } from "../cli/prompt.js";
import type { Command, CommandArgs } from "../cli/registry.js";

async function login(context: CommandContext, args: CommandArgs): Promise<void> {
  const endpoint = context.globals.endpoint ?? (await context.loadProfile())?.endpoint;
  if (endpoint === undefined) {
    throw usageError("pass --endpoint <url> (or set MAKO_ENDPOINT) the first time you sign in");
  }
  const email = args.string("email") ?? (await promptLine(context.io, "Email: "));
  if (!email.includes("@")) throw usageError("an email address is required");
  const passwordFile = args.string("password-file");
  const password =
    passwordFile !== undefined
      ? (await context.readInput(`@${passwordFile}`)).replace(/\r?\n$/u, "")
      : await promptSecret(context.io, "Password: ");
  const { client, jar } = context.developerAuth(endpoint);
  const session = await client.signIn(email, password);
  if (session.status === "waitlisted" || session.audience !== "mako-management") {
    throw new CliError(
      "your registration is on the wait-list; the console will tell you when it is approved",
      EXIT.auth,
      "CLI_WAITLISTED",
    );
  }
  const stored: StoredSession = {
    accessToken: session.accessToken,
    expiresAt: session.expiresAt,
    audience: session.audience,
    email,
    ...(jar.value !== undefined ? { refreshCookie: jar.value } : {}),
  };
  await context.saveProfile({ endpoint, session: stored });
  const record = {
    profile: context.globals.profile,
    endpoint,
    email,
    expiresAt: session.expiresAt,
  };
  if (context.json) context.out(record);
  else context.info(`Signed in as ${email} at ${endpoint} (profile "${context.globals.profile}").`);
}

async function logout(context: CommandContext): Promise<void> {
  const profile = await context.loadProfile();
  if (profile?.session === undefined) {
    context.info("Not signed in.");
    return;
  }
  if (profile.session.refreshCookie !== undefined) {
    const { client } = context.developerAuth(profile.endpoint, profile.session.refreshCookie);
    try {
      await client.signOut();
    } catch (error) {
      if (
        !(error instanceof ManagementApiError && (error.status === 401 || error.status === 403))
      ) {
        throw error;
      }
    }
  }
  await context.saveProfile({ endpoint: profile.endpoint });
  context.info("Signed out; the session was revoked and removed from this machine.");
}

async function status(context: CommandContext): Promise<void> {
  const profile = await context.loadProfile();
  const record: Record<string, unknown> = {
    profile: context.globals.profile,
    endpoint: context.globals.endpoint ?? profile?.endpoint ?? null,
    credential: context.usesEnvironmentToken
      ? "environment (MAKO_TOKEN)"
      : profile?.session !== undefined
        ? "stored developer session"
        : "none",
    email: profile?.session?.email ?? null,
    expiresAt: profile?.session?.expiresAt ?? null,
    renewable: profile?.session?.refreshCookie !== undefined,
  };
  context.out(record);
}

async function whoami(context: CommandContext): Promise<void> {
  const client = await context.management();
  const teams = await client.listTeams();
  const personal = teams.find((team) => (team as { kind?: string }).kind === "personal");
  const profile = await context.loadProfile();
  const record = {
    email: context.usesEnvironmentToken ? null : (profile?.session?.email ?? null),
    credential: context.usesEnvironmentToken ? "MAKO_TOKEN" : "developer session",
    personalSpaceId: personal?.id ?? null,
    teams: teams.map((team) => ({ id: team.id, name: team.name })),
  };
  context.out(record);
}

export const authCommands: readonly Command[] = [
  {
    path: ["auth", "login"],
    summary: "Sign in with email and password and store the session for this profile",
    operations: ["createDeveloperSession"],
    options: {
      email: {
        type: "string",
        description: "Email address (prompted if omitted)",
        placeholder: "<email>",
      },
      "password-file": {
        type: "string",
        description: "Read the password from a file instead of prompting",
        placeholder: "<path>",
      },
    },
    run: login,
  },
  {
    path: ["auth", "logout"],
    summary: "Revoke the stored session server-side and remove it",
    operations: ["deleteDeveloperSession"],
    run: (context) => logout(context),
  },
  {
    path: ["auth", "status"],
    summary: "Show which credential and endpoint commands will use",
    operations: ["refreshDeveloperSession"],
    run: (context) => status(context),
  },
  {
    path: ["auth", "whoami"],
    summary: "Show the signed-in developer, their personal space, and teams",
    operations: [],
    run: (context) => whoami(context),
  },
];

export { authError };
