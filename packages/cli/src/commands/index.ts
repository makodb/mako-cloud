import type { Command } from "../cli/registry.js";
import { authCommands } from "./auth.js";
import { authRegistrationCommands } from "./auth-registration.js";
import { authSettingsCommands } from "./auth-settings.js";
import { authTokensCommands } from "./auth-tokens.js";
import { collectionsCommands } from "./collections.js";
import { dataCommands } from "./data.js";
import { emailTemplatesCommands } from "./email-templates.js";
import { envsCommands } from "./envs.js";
import { explorerCommands } from "./explorer.js";
import { functionsCommands } from "./functions.js";
import { functionsDeployCommands } from "./functions-deploy.js";
import { indexesCommands } from "./indexes.js";
import { keysCommands } from "./keys.js";
import { observabilityCommands } from "./observability.js";
import { policiesCommands } from "./policies.js";
import { projectsCommands } from "./projects.js";
import { storageCommands } from "./storage.js";
import { teamsCommands } from "./teams.js";
import { usersCommands } from "./users.js";
import { workspaceCommands } from "./workspace.js";

/** `mako functions serve` is dispatched before the registry; it is listed for help and parity. */
const serveCommand: Command = {
  path: ["functions", "serve"],
  summary: "Run a function locally in the pinned edge runtime",
  operations: [],
  run: () => Promise.reject(new Error("functions serve is dispatched before the registry")),
};

export const commands: readonly Command[] = [
  ...authCommands,
  ...authRegistrationCommands,
  ...authTokensCommands,
  ...teamsCommands,
  ...projectsCommands,
  ...envsCommands,
  ...collectionsCommands,
  ...indexesCommands,
  ...policiesCommands,
  ...usersCommands,
  ...keysCommands,
  ...authSettingsCommands,
  ...functionsCommands,
  ...functionsDeployCommands,
  serveCommand,
  ...observabilityCommands,
  ...workspaceCommands,
  ...explorerCommands,
  ...dataCommands,
  ...storageCommands,
  ...emailTemplatesCommands,
];
