import type { CommandContext } from "../cli/context.js";
import { authError, usageError } from "../cli/errors.js";
import { CLI_NAME } from "../cli/name.js";
import { promptLine, promptSecret } from "../cli/prompt.js";
import type { Command, CommandArgs, OptionSpec } from "../cli/registry.js";

/** Registration flows run before any session exists, so only the endpoint is needed. */
async function developerAuthEndpoint(context: CommandContext): Promise<string> {
  const endpoint = context.globals.endpoint ?? (await context.loadProfile())?.endpoint;
  if (endpoint === undefined) {
    throw usageError("pass --endpoint <url> or set MAKO_ENDPOINT: no endpoint is stored yet");
  }
  return endpoint;
}

/** A password comes from a 0600 file or an echo-off prompt; never from an argument. */
async function passwordFrom(
  context: CommandContext,
  args: CommandArgs,
  question: string,
): Promise<string> {
  const file = args.string("password-file");
  if (file !== undefined) {
    return (await context.readInput(`@${file}`)).replace(/\r?\n$/u, "");
  }
  return promptSecret(context.io, question);
}

async function emailFrom(context: CommandContext, args: CommandArgs): Promise<string> {
  const email = args.string("email") ?? (await promptLine(context.io, "Email: "));
  if (!email.includes("@")) throw usageError("an email address is required");
  return email;
}

/** JSON gets the API's response; a terminal gets the sentence that matters. */
function report(context: CommandContext, value: unknown, line: string): void {
  if (context.json) context.out(value);
  else context.io.stdout.write(`${line}\n`);
}

function statusLine(status: "waitlisted" | "password_updated"): string {
  return status === "password_updated"
    ? `Password updated. Sign in with \`${CLI_NAME} auth login\`.`
    : "Email verified. Your registration is on the wait-list; you will hear when it is approved.";
}

const EMAIL_OPTION: Readonly<Record<string, OptionSpec>> = {
  email: { type: "string", description: "Email address", placeholder: "<email>", required: true },
};

const PASSWORD_FILE_OPTION: Readonly<Record<string, OptionSpec>> = {
  "password-file": {
    type: "string",
    description: "Read the password from a file instead of prompting",
    placeholder: "<path>",
  },
};

async function register(context: CommandContext, args: CommandArgs): Promise<void> {
  const endpoint = await developerAuthEndpoint(context);
  const email = await emailFrom(context, args);
  const displayName =
    args.string("display-name") ?? (await promptLine(context.io, "Display name: "));
  if (displayName.trim() === "") throw usageError("a display name is required");
  const password = await passwordFrom(context, args, "Password: ");
  const { client } = context.developerAuth(endpoint);
  const accepted = await client.register({ email, displayName: displayName.trim(), password });
  report(context, accepted, accepted.message);
}

async function waitListStatus(context: CommandContext, args: CommandArgs): Promise<void> {
  const endpoint = await developerAuthEndpoint(context);
  const { client } = context.developerAuth(endpoint);
  const email = args.string("email");
  if (email !== undefined) {
    // `mako-cloud auth login` refuses to store a wait-listed session, so the token
    // that can answer this question is obtained here and kept in memory only.
    if (!email.includes("@")) throw usageError("an email address is required");
    const password = await passwordFrom(context, args, "Password: ");
    const session = await client.signIn(email, password);
    if (session.status !== "waitlisted") {
      report(
        context,
        { status: session.status, email },
        `Your registration was approved; sign in with \`${CLI_NAME} auth login\`.`,
      );
      return;
    }
    context.out(await client.waitListStatus(session.accessToken));
    return;
  }
  const environmentToken = context.io.env.MAKO_TOKEN;
  const token =
    environmentToken !== undefined && environmentToken !== ""
      ? environmentToken
      : (await context.loadProfile())?.session?.accessToken;
  if (token === undefined) {
    throw authError(
      "no wait-list token: pass --email to check with your password, or set MAKO_TOKEN",
    );
  }
  context.out(await client.waitListStatus(token));
}

export const authRegistrationCommands: readonly Command[] = [
  {
    path: ["auth", "register"],
    summary: "Register a developer account with the hosted registration flow",
    operations: ["registerDeveloper"],
    options: {
      email: {
        type: "string",
        description: "Email address (prompted if omitted)",
        placeholder: "<email>",
      },
      "display-name": {
        type: "string",
        description: "Name shown to teammates (prompted if omitted)",
        placeholder: "<name>",
      },
      ...PASSWORD_FILE_OPTION,
    },
    run: register,
  },
  {
    path: ["auth", "verify-email"],
    summary: "Confirm an email address with the token from the verification mail",
    operations: ["verifyDeveloperEmail"],
    positionals: [{ name: "token", description: "Verification token", required: true }],
    run: async (context, args) => {
      const token = args.requirePositional(0, "token");
      const { client } = context.developerAuth(await developerAuthEndpoint(context));
      const result = await client.verifyEmail(token);
      report(context, result, statusLine(result.status));
    },
  },
  {
    path: ["auth", "resend-verification"],
    summary: "Send the verification mail again",
    operations: ["resendDeveloperVerification"],
    options: EMAIL_OPTION,
    run: async (context, args) => {
      const email = args.requireString("email");
      const { client } = context.developerAuth(await developerAuthEndpoint(context));
      const accepted = await client.resendVerification(email);
      report(context, accepted, accepted.message);
    },
  },
  {
    path: ["auth", "recover-password"],
    summary: "Send a password recovery mail",
    operations: ["requestDeveloperPasswordRecovery"],
    options: EMAIL_OPTION,
    run: async (context, args) => {
      const email = args.requireString("email");
      const { client } = context.developerAuth(await developerAuthEndpoint(context));
      const accepted = await client.requestPasswordRecovery(email);
      report(context, accepted, accepted.message);
    },
  },
  {
    path: ["auth", "reset-password"],
    summary: "Set a new password with the token from the recovery mail",
    operations: ["completeDeveloperPasswordRecovery"],
    positionals: [{ name: "token", description: "Recovery token", required: true }],
    options: PASSWORD_FILE_OPTION,
    run: async (context, args) => {
      const token = args.requirePositional(0, "token");
      const endpoint = await developerAuthEndpoint(context);
      const password = await passwordFrom(context, args, "New password: ");
      const { client } = context.developerAuth(endpoint);
      const result = await client.completePasswordRecovery(token, password);
      report(context, result, statusLine(result.status));
    },
  },
  {
    path: ["auth", "waitlist-status"],
    summary: "Show whether a registration is still on the wait-list",
    operations: ["getDeveloperWaitListStatus", "createDeveloperSession"],
    options: {
      email: {
        type: "string",
        description: "Sign in with this address to check (the session is not stored)",
        placeholder: "<email>",
      },
      ...PASSWORD_FILE_OPTION,
    },
    run: waitListStatus,
  },
];
