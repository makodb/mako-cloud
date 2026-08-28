import { CommandContext, type CommandIo, globalsFrom } from "./context.js";
import { exitCodeFor, renderError } from "./errors.js";
import { type Command, CommandRegistry } from "./registry.js";
import { parseServeArguments, runLocalServe } from "../serve.js";

/** Runs one invocation and returns its exit code; never throws. */
export async function run(
  argv: readonly string[],
  io: CommandIo,
  commands: readonly Command[],
): Promise<number> {
  if (argv[0] === "functions" && argv[1] === "serve") {
    try {
      await runLocalServe(parseServeArguments([...argv], io.cwd));
      return 0;
    } catch (error) {
      io.stderr.write(
        `${error instanceof Error ? error.message : "local function serve failed"}\n`,
      );
      return 1;
    }
  }
  try {
    const registry = new CommandRegistry(commands);
    const resolution = registry.resolve(argv);
    if (resolution.kind === "help") {
      io.stdout.write(resolution.text);
      return 0;
    }
    const { command, rest } = resolution;
    const parsed = registry.parse(command, rest);
    if (parsed.help) {
      io.stdout.write(registry.help(command.path));
      return 0;
    }
    const context = new CommandContext(io, globalsFrom(parsed.args, io.env));
    if (command.destructive !== undefined) {
      await context.confirmDestructive(
        command.destructive.action,
        command.destructive.resource(parsed.args),
      );
    }
    await command.run(context, parsed.args);
    return 0;
  } catch (error) {
    io.stderr.write(`${renderError(error)}\n`);
    return exitCodeFor(error);
  }
}

export function processIo(): CommandIo {
  return {
    stdout: process.stdout,
    stderr: process.stderr,
    stdin: process.stdin,
    isTTY: Boolean(process.stdin.isTTY && process.stderr.isTTY),
    env: process.env,
    cwd: process.cwd(),
  };
}
