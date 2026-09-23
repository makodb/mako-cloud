import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import { setTimeout as delay } from "node:timers/promises";

import {
  type ManagementCredential,
  MakoDeveloperAuthClient,
  MakoManagementClient,
  ManagementApiError,
} from "@mako-cloud/management-sdk";

import {
  loadStore,
  type Profile,
  REFRESH_COOKIE_NAME,
  refreshCookieFrom,
  resolveConfigDir,
  saveStore,
  type StoredSession,
  withProfile,
} from "./credentials.js";
import { authError, CliError, EXIT, usageError } from "./errors.js";
import { CLI_NAME } from "./name.js";
import {
  type OutputSink,
  renderRecord,
  renderSecretBlock,
  renderTable,
  type TableColumn,
} from "./output.js";
import { promptLine, promptSecret } from "./prompt.js";
import type { CommandArgs } from "./registry.js";

/** Everything a command touches outside the process: streams, environment, clock. */
export interface CommandIo {
  readonly stdout: OutputSink;
  readonly stderr: OutputSink;
  readonly stdin: NodeJS.ReadableStream | null;
  readonly isTTY: boolean;
  readonly env: Readonly<Record<string, string | undefined>>;
  readonly cwd: string;
  readonly fetch?: typeof globalThis.fetch;
}

export interface GlobalValues {
  readonly endpoint: string | undefined;
  readonly profile: string;
  readonly configDir: string;
  readonly json: boolean;
  readonly all: boolean;
  readonly yes: boolean;
  readonly wait: boolean;
  readonly timeoutMs: number;
}

export interface Page<T> {
  readonly items: readonly T[];
  readonly nextCursor?: string | null;
}

export interface OutputView {
  readonly columns?: readonly TableColumn[];
  /** Renders a human line per item instead of a table. */
  readonly line?: (item: unknown) => string;
}

const REFRESH_MARGIN_MS = 60_000;
const DEFAULT_PROFILE = "default";

export function globalsFrom(
  args: CommandArgs,
  env: Readonly<Record<string, string | undefined>>,
): GlobalValues {
  const timeoutSeconds = args.integer("timeout") ?? 600;
  if (timeoutSeconds <= 0) throw usageError("--timeout must be a positive number of seconds");
  const configDir = args.string("config-dir") ?? resolveConfigDir(env);
  const endpoint = args.string("endpoint") ?? env.MAKO_ENDPOINT;
  return {
    endpoint: endpoint !== undefined && endpoint !== "" ? endpoint : undefined,
    profile: args.string("profile") ?? env.MAKO_PROFILE ?? DEFAULT_PROFILE,
    configDir,
    json: args.boolean("json"),
    all: args.boolean("all"),
    yes: args.boolean("yes"),
    wait: args.boolean("wait"),
    timeoutMs: timeoutSeconds * 1000,
  };
}

function isPage(value: unknown): value is Page<unknown> {
  return (
    typeof value === "object" &&
    value !== null &&
    Array.isArray((value as { items?: unknown }).items)
  );
}

/** A cookie jar of one cookie: the developer refresh cookie the API sets. */
export interface RefreshCookieJar {
  value: string | undefined;
}

export class CommandContext {
  readonly io: CommandIo;
  readonly globals: GlobalValues;
  #management: MakoManagementClient | undefined;
  #resolvedEndpoint: string | undefined;

  constructor(io: CommandIo, globals: GlobalValues) {
    this.io = io;
    this.globals = globals;
  }

  get json(): boolean {
    return this.globals.json;
  }

  // ---- output -------------------------------------------------------------

  /** Prints a value: JSON verbatim with --json, otherwise a table, record, or scalar. */
  out(value: unknown, view: OutputView = {}): void {
    if (this.globals.json) {
      this.io.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
      return;
    }
    this.io.stdout.write(this.#human(value, view));
  }

  #human(value: unknown, view: OutputView): string {
    if (Array.isArray(value)) {
      if (view.line) return value.map((item) => `${view.line?.(item)}\n`).join("") || "(none)\n";
      return renderTable(value as Record<string, unknown>[], view.columns);
    }
    if (isPage(value)) {
      const body = this.#human([...value.items], view);
      const cursor = value.nextCursor;
      return cursor ? `${body}next cursor: ${cursor}\n` : body;
    }
    if (typeof value === "object" && value !== null) {
      return renderRecord(value as Record<string, unknown>);
    }
    return `${String(value)}\n`;
  }

  /** A progress or status line; never part of the parseable output. */
  info(message: string): void {
    this.io.stderr.write(`${message}\n`);
  }

  /**
   * Prints a one-time secret exactly once: as JSON with --json, into a 0600 file
   * with --secret-file, or as a delimited block. The rest of the record goes to
   * the ordinary output with the secret removed.
   */
  async secret(
    label: string,
    secret: string,
    record: Record<string, unknown>,
    secretFile: string | undefined,
  ): Promise<void> {
    if (secretFile !== undefined) {
      const { writeFile } = await import("node:fs/promises");
      await writeFile(secretFile, `${secret}\n`, { mode: 0o600 });
      this.info(`${label} written to ${secretFile}`);
      this.out(record);
      return;
    }
    if (this.globals.json) {
      this.out({ ...record, secret });
      return;
    }
    this.io.stdout.write(renderRecord(record));
    this.io.stdout.write(renderSecretBlock(label, secret));
  }

  // ---- endpoint and credentials -----------------------------------------

  /** The endpoint in force: --endpoint, MAKO_ENDPOINT, then the profile's. */
  async endpoint(): Promise<string> {
    if (this.#resolvedEndpoint !== undefined) return this.#resolvedEndpoint;
    if (this.globals.endpoint !== undefined) {
      this.#resolvedEndpoint = this.globals.endpoint;
      return this.#resolvedEndpoint;
    }
    const profile = await this.loadProfile();
    if (profile?.endpoint) {
      this.#resolvedEndpoint = profile.endpoint;
      return this.#resolvedEndpoint;
    }
    throw usageError(
      `no endpoint: pass --endpoint, set MAKO_ENDPOINT, or run \`${CLI_NAME} auth login --endpoint <url>\``,
    );
  }

  async loadProfile(): Promise<Profile | undefined> {
    const store = await loadStore(this.globals.configDir);
    return store.profiles[this.globals.profile];
  }

  async saveProfile(profile: Profile | undefined): Promise<void> {
    const store = await loadStore(this.globals.configDir);
    await saveStore(this.globals.configDir, withProfile(store, this.globals.profile, profile));
  }

  /** Whether the credential comes from the environment rather than the store. */
  get usesEnvironmentToken(): boolean {
    const token = this.io.env.MAKO_TOKEN;
    return token !== undefined && token !== "";
  }

  /** The bearer credential for management calls; renews a stored session when due. */
  async credential(): Promise<ManagementCredential> {
    const token = this.io.env.MAKO_TOKEN;
    if (token !== undefined && token !== "") {
      if (token.length < 16 || /\s/u.test(token)) {
        throw authError("MAKO_TOKEN is not a valid token");
      }
      const kind =
        this.io.env.MAKO_TOKEN_KIND === "developer_session"
          ? "developer_session"
          : "automation_token";
      return { kind, accessToken: token };
    }
    const profile = await this.loadProfile();
    if (profile?.session === undefined) {
      throw authError(
        `not signed in (profile "${this.globals.profile}"): run \`${CLI_NAME} auth login\` or set MAKO_TOKEN`,
      );
    }
    const explicit = this.globals.endpoint;
    if (explicit !== undefined && normalize(explicit) !== normalize(profile.endpoint)) {
      throw authError(
        `profile "${this.globals.profile}" is signed in to ${profile.endpoint}, not ${explicit}; use another --profile or MAKO_TOKEN`,
      );
    }
    let session = profile.session;
    if (Date.parse(session.expiresAt) - Date.now() < REFRESH_MARGIN_MS) {
      session = await this.#renew(profile, session);
    }
    return { kind: "developer_session", accessToken: session.accessToken };
  }

  async #renew(profile: Profile, session: StoredSession): Promise<StoredSession> {
    if (session.refreshCookie === undefined) {
      throw authError(
        `the stored session has expired and cannot be renewed: run \`${CLI_NAME} auth login\``,
      );
    }
    const { client, jar } = this.developerAuth(profile.endpoint, session.refreshCookie);
    let renewed: Awaited<ReturnType<MakoDeveloperAuthClient["refresh"]>>;
    try {
      renewed = await client.refresh();
    } catch (error) {
      if (error instanceof ManagementApiError && (error.status === 401 || error.status === 403)) {
        throw authError(`the stored session could not be renewed: run \`${CLI_NAME} auth login\``);
      }
      throw error;
    }
    const next: StoredSession = {
      accessToken: renewed.accessToken,
      expiresAt: renewed.expiresAt,
      audience: renewed.audience,
      ...(session.email !== undefined ? { email: session.email } : {}),
      ...(jar.value !== undefined ? { refreshCookie: jar.value } : {}),
    };
    await this.saveProfile({ ...profile, session: next });
    return next;
  }

  /** The management client, built once per command. */
  async management(): Promise<MakoManagementClient> {
    if (this.#management !== undefined) return this.#management;
    const endpoint = await this.endpoint();
    const credential = await this.credential();
    this.#management = new MakoManagementClient({
      endpoint,
      credential,
      fetch: this.#originFetch(endpoint, undefined),
    });
    return this.#management;
  }

  /**
   * A developer-auth client whose requests carry the endpoint as Origin (the
   * API's cross-site check) and the refresh cookie, and which captures the
   * cookie the API sets.
   */
  developerAuth(
    endpoint: string,
    refreshCookie?: string,
  ): { readonly client: MakoDeveloperAuthClient; readonly jar: RefreshCookieJar } {
    const jar: RefreshCookieJar = { value: refreshCookie };
    const client = new MakoDeveloperAuthClient({
      endpoint,
      fetch: this.#originFetch(endpoint, jar),
    });
    return { client, jar };
  }

  #originFetch(endpoint: string, jar: RefreshCookieJar | undefined): typeof globalThis.fetch {
    const base = this.io.fetch ?? globalThis.fetch;
    const origin = new URL(endpoint).origin;
    return async (input, init) => {
      const request = new Request(input, init);
      request.headers.set("Origin", origin);
      if (jar?.value !== undefined) {
        request.headers.set("Cookie", `${REFRESH_COOKIE_NAME}=${jar.value}`);
      }
      const response = await base(request);
      if (jar !== undefined) {
        const headers = response.headers as Headers & { getSetCookie?: () => string[] };
        const raw = headers.getSetCookie
          ? headers.getSetCookie().join(", ")
          : headers.get("set-cookie");
        const value = refreshCookieFrom(raw);
        if (value !== undefined) jar.value = value;
      }
      return response;
    };
  }

  /**
   * A step-up grant token for actions the API gates behind a fresh password:
   * prompted on a terminal, or read from MAKO_STEP_UP_PASSWORD_FILE.
   */
  async stepUp(): Promise<string> {
    const file = this.io.env.MAKO_STEP_UP_PASSWORD_FILE;
    let password: string;
    if (file !== undefined && file !== "") {
      password = (await readFile(file, "utf8")).replace(/\r?\n$/u, "");
    } else if (this.io.isTTY) {
      password = await promptSecret(this.io, "Password (step-up verification): ");
    } else {
      throw authError(
        "this action needs a step-up verification: run it on a terminal, or set MAKO_STEP_UP_PASSWORD_FILE",
      );
    }
    const client = await this.management();
    const grant = await client.verifyCurrentDeveloperPassword(password);
    return grant.token;
  }

  // ---- safety -------------------------------------------------------------

  /** Refuses a destructive action unless --yes was given or the resource name is typed. */
  async confirmDestructive(action: string, resource: string): Promise<void> {
    if (this.globals.yes) return;
    if (!this.io.isTTY) {
      throw usageError(`refusing to ${action} ${resource} without --yes`);
    }
    const typed = await promptLine(this.io, `Type "${resource}" to ${action}: `);
    if (typed.trim() !== resource) {
      throw new CliError(
        `confirmation did not match "${resource}"; nothing was done`,
        EXIT.usage,
        "CLI_NOT_CONFIRMED",
      );
    }
  }

  idempotencyKey(): string {
    return randomUUID();
  }

  // ---- waiting and paging -------------------------------------------------

  /** Polls until `terminal` holds or the deadline passes (exit code 6). */
  async waitFor<T>(
    poll: () => Promise<T>,
    terminal: (value: T) => boolean,
    describe: (value: T) => string,
  ): Promise<T> {
    const interval = Number.parseInt(this.io.env.MAKO_WAIT_INTERVAL_MS ?? "2000", 10);
    const deadline = Date.now() + this.globals.timeoutMs;
    let last = await poll();
    let described = describe(last);
    this.info(described);
    while (!terminal(last)) {
      if (Date.now() >= deadline) {
        throw new CliError(
          `timed out waiting; last observed: ${described}`,
          EXIT.timeout,
          "CLI_WAIT_TIMEOUT",
        );
      }
      await delay(interval);
      last = await poll();
      const next = describe(last);
      if (next !== described) {
        described = next;
        this.info(described);
      }
    }
    return last;
  }

  /** One page, or every page with --all, as a single page value. */
  async collect<T>(fetchPage: (cursor: string | undefined) => Promise<Page<T>>): Promise<Page<T>> {
    let page = await fetchPage(undefined);
    if (!this.globals.all) return page;
    const items = [...page.items];
    while (page.nextCursor) {
      page = await fetchPage(page.nextCursor);
      items.push(...page.items);
    }
    return { items, nextCursor: null };
  }

  /** Reads a body from `-` (stdin), `@path`, or an inline value. */
  async readInput(source: string): Promise<string> {
    if (source === "-") {
      if (this.io.stdin === null) throw usageError("no stdin to read from");
      const chunks: Buffer[] = [];
      for await (const chunk of this.io.stdin) {
        chunks.push(typeof chunk === "string" ? Buffer.from(chunk) : chunk);
      }
      return Buffer.concat(chunks).toString("utf8");
    }
    if (source.startsWith("@")) return readFile(source.slice(1), "utf8");
    return source;
  }

  /** Parses JSON from `-`, `@path`, or inline text with a usage error on failure. */
  async readJson<T = unknown>(source: string, what: string): Promise<T> {
    const text = await this.readInput(source);
    try {
      return JSON.parse(text) as T;
    } catch {
      throw usageError(`${what} is not valid JSON`);
    }
  }
}

function normalize(endpoint: string): string {
  return endpoint.replace(/\/+$/u, "").toLowerCase();
}
