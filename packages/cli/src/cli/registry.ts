import { parseArgs } from "node:util";

import { usageError } from "./errors.js";
import type { CommandContext } from "./context.js";

export type OptionType = "string" | "boolean";

export interface OptionSpec {
  readonly type: OptionType;
  readonly description: string;
  readonly multiple?: boolean;
  readonly short?: string;
  readonly required?: boolean;
  /** Shown in help as the value name, e.g. `<name>`. */
  readonly placeholder?: string;
}

export interface PositionalSpec {
  readonly name: string;
  readonly description: string;
  readonly required?: boolean;
  readonly variadic?: boolean;
}

export type OptionValue = string | boolean | readonly string[] | undefined;

/** Parsed arguments with typed accessors; unknown names are programming errors. */
export class CommandArgs {
  readonly positionals: readonly string[];
  readonly values: Readonly<Record<string, OptionValue>>;

  constructor(positionals: readonly string[], values: Readonly<Record<string, OptionValue>>) {
    this.positionals = positionals;
    this.values = values;
  }

  string(name: string): string | undefined {
    const value = this.values[name];
    if (Array.isArray(value)) return value[value.length - 1];
    return typeof value === "string" ? value : undefined;
  }

  requireString(name: string): string {
    const value = this.string(name);
    if (value === undefined || value === "") throw usageError(`--${name} is required`);
    return value;
  }

  strings(name: string): readonly string[] {
    const value = this.values[name];
    if (Array.isArray(value)) return value as readonly string[];
    return typeof value === "string" ? [value] : [];
  }

  boolean(name: string): boolean {
    return this.values[name] === true;
  }

  integer(name: string): number | undefined {
    const value = this.string(name);
    if (value === undefined) return undefined;
    if (!/^-?\d+$/u.test(value)) throw usageError(`--${name} must be an integer`);
    return Number.parseInt(value, 10);
  }

  positional(index: number): string | undefined {
    return this.positionals[index];
  }

  requirePositional(index: number, name: string): string {
    const value = this.positionals[index];
    if (value === undefined || value === "") throw usageError(`<${name}> is required`);
    return value;
  }
}

export interface Command {
  /** Words that select the command, e.g. `["teams", "create"]`. */
  readonly path: readonly string[];
  readonly summary: string;
  /** OpenAPI operation ids this command reaches; the parity test reads them. */
  readonly operations: readonly string[];
  readonly positionals?: readonly PositionalSpec[];
  readonly options?: Readonly<Record<string, OptionSpec>>;
  /** Present on commands that must be confirmed before anything is sent. */
  readonly destructive?: {
    readonly action: string;
    readonly resource: (args: CommandArgs) => string;
  };
  run(context: CommandContext, args: CommandArgs): Promise<void>;
}

export const GLOBAL_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  endpoint: {
    type: "string",
    description: "Management API endpoint; also MAKO_ENDPOINT",
    placeholder: "<url>",
  },
  profile: {
    type: "string",
    description: "Credential profile (default: default)",
    placeholder: "<name>",
  },
  "config-dir": {
    type: "string",
    description: "Credential store directory; also MAKO_CONFIG_DIR",
    placeholder: "<path>",
  },
  json: { type: "boolean", description: "Print the API response as JSON" },
  all: { type: "boolean", description: "Follow pagination and print every page" },
  yes: { type: "boolean", short: "y", description: "Confirm destructive actions without a prompt" },
  wait: { type: "boolean", description: "Wait for a lifecycle change to reach a terminal state" },
  timeout: {
    type: "string",
    description: "Seconds to wait with --wait (default: 600)",
    placeholder: "<seconds>",
  },
  help: { type: "boolean", short: "h", description: "Show help" },
};

export type Resolution =
  | { readonly kind: "command"; readonly command: Command; readonly rest: readonly string[] }
  | { readonly kind: "help"; readonly text: string };

export interface ParsedCommand {
  readonly args: CommandArgs;
  readonly help: boolean;
}

function parseOptionsFor(
  options: Readonly<Record<string, OptionSpec>>,
): NonNullable<Parameters<typeof parseArgs>[0]>["options"] {
  const result: Record<string, { type: OptionType; multiple?: boolean; short?: string }> = {};
  for (const [name, spec] of Object.entries(options)) {
    const entry: { type: OptionType; multiple?: boolean; short?: string } = { type: spec.type };
    if (spec.multiple) entry.multiple = true;
    if (spec.short !== undefined) entry.short = spec.short;
    result[name] = entry;
  }
  return result;
}

/** Owns the command tree: resolution by longest matching path, parsing, and help. */
export class CommandRegistry {
  readonly commands: readonly Command[];
  readonly #byPath = new Map<string, Command>();

  constructor(commands: readonly Command[]) {
    for (const command of commands) {
      const key = command.path.join(" ");
      if (this.#byPath.has(key)) throw new Error(`duplicate command: ${key}`);
      this.#byPath.set(key, command);
    }
    this.commands = [...commands].sort((a, b) => a.path.join(" ").localeCompare(b.path.join(" ")));
  }

  resolve(argv: readonly string[]): Resolution {
    const words: string[] = [];
    for (const token of argv) {
      if (token.startsWith("-")) break;
      const candidate = [...words, token].join(" ");
      if (!this.#hasPrefix(candidate)) break;
      words.push(token);
    }
    const command = this.#byPath.get(words.join(" "));
    if (command !== undefined) {
      return { kind: "command", command, rest: argv.slice(words.length) };
    }
    const wantsHelp = argv.includes("--help") || argv.includes("-h");
    const unknown = argv.find((token) => !token.startsWith("-") && !words.includes(token));
    if (!wantsHelp && unknown !== undefined && words.length > 0) {
      throw usageError(`unknown command: mako ${[...words, unknown].join(" ")}`);
    }
    if (!wantsHelp && unknown !== undefined) {
      throw usageError(`unknown command: mako ${unknown}`);
    }
    return { kind: "help", text: this.help(words) };
  }

  #hasPrefix(candidate: string): boolean {
    for (const key of this.#byPath.keys()) {
      if (key === candidate || key.startsWith(`${candidate} `)) return true;
    }
    return false;
  }

  parse(command: Command, rest: readonly string[]): ParsedCommand {
    const options = { ...GLOBAL_OPTIONS, ...(command.options ?? {}) };
    let parsed: ReturnType<typeof parseArgs>;
    try {
      parsed = parseArgs({
        args: [...rest],
        options: parseOptionsFor(options),
        allowPositionals: true,
        strict: true,
      });
    } catch (error) {
      throw usageError(
        `${error instanceof Error ? error.message : String(error)} (see: mako ${command.path.join(" ")} --help)`,
      );
    }
    const values = parsed.values as Record<string, OptionValue>;
    const help = values.help === true;
    if (!help) {
      const specs = command.positionals ?? [];
      const required = specs.filter((spec) => spec.required).length;
      if (parsed.positionals.length < required) {
        const missing = specs[parsed.positionals.length];
        throw usageError(
          `<${missing?.name ?? "argument"}> is required (see: mako ${command.path.join(" ")} --help)`,
        );
      }
      const variadic = specs.some((spec) => spec.variadic);
      if (!variadic && parsed.positionals.length > specs.length) {
        throw usageError(
          `unexpected argument: ${parsed.positionals[specs.length]} (see: mako ${command.path.join(" ")} --help)`,
        );
      }
      for (const [name, spec] of Object.entries(command.options ?? {})) {
        if (spec.required && values[name] === undefined) throw usageError(`--${name} is required`);
      }
    }
    return { args: new CommandArgs(parsed.positionals, values), help };
  }

  /** Help for a command, or the listing under a group prefix (empty for the root). */
  help(path: readonly string[]): string {
    const exact = this.#byPath.get(path.join(" "));
    if (exact !== undefined) return renderCommandHelp(exact);
    const prefix = path.length === 0 ? "" : `${path.join(" ")} `;
    const groups = new Map<string, string[]>();
    for (const command of this.commands) {
      const key = command.path.join(" ");
      if (prefix !== "" && !key.startsWith(prefix)) continue;
      const remainder = command.path.slice(path.length);
      const head = remainder[0];
      if (head === undefined) continue;
      const list = groups.get(head) ?? [];
      list.push(
        remainder.length === 1
          ? command.summary
          : `${remainder.slice(1).join(" ")}: ${command.summary}`,
      );
      groups.set(head, list);
    }
    const lines = [`usage: mako ${prefix}<command> [options]`, ""];
    const width = Math.max(...[...groups.keys()].map((key) => key.length), 4);
    for (const [head, entries] of groups) {
      if (entries.length === 1 && !entries[0]?.includes(": ")) {
        lines.push(`  ${head.padEnd(width)}  ${entries[0]}`);
      } else {
        lines.push(`  ${head.padEnd(width)}  ${entries.length} commands`);
      }
    }
    lines.push("", `Run "mako ${prefix}<command> --help" for details.`);
    return `${lines.join("\n")}\n`;
  }
}

function renderOptions(options: Readonly<Record<string, OptionSpec>>): string[] {
  const entries = Object.entries(options);
  if (entries.length === 0) return [];
  const names = entries.map(([name, spec]) => {
    const short = spec.short !== undefined ? `-${spec.short}, ` : "";
    const value = spec.type === "string" ? ` ${spec.placeholder ?? "<value>"}` : "";
    return `${short}--${name}${value}`;
  });
  const width = Math.max(...names.map((name) => name.length));
  return entries.map(
    ([, spec], index) =>
      `  ${(names[index] ?? "").padEnd(width)}  ${spec.description}${spec.required ? " (required)" : ""}`,
  );
}

export function renderCommandHelp(command: Command): string {
  const positionals = (command.positionals ?? [])
    .map(
      (spec) =>
        (spec.required ? `<${spec.name}>` : `[${spec.name}]`) + (spec.variadic ? "..." : ""),
    )
    .join(" ");
  const lines = [
    `usage: mako ${command.path.join(" ")}${positionals ? ` ${positionals}` : ""} [options]`,
    "",
    command.summary,
  ];
  if (command.positionals?.length) {
    lines.push("", "arguments:");
    const width = Math.max(...command.positionals.map((spec) => spec.name.length));
    for (const spec of command.positionals) {
      lines.push(`  ${spec.name.padEnd(width)}  ${spec.description}`);
    }
  }
  if (command.options && Object.keys(command.options).length > 0) {
    lines.push("", "options:", ...renderOptions(command.options));
  }
  if (command.destructive) {
    lines.push("", `Confirmation: pass --yes or type the resource name when asked.`);
  }
  lines.push("", "global options:", ...renderOptions(GLOBAL_OPTIONS));
  return `${lines.join("\n")}\n`;
}
