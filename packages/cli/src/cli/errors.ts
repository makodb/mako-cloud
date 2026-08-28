import { ManagementApiError } from "@mako-cloud/management-sdk";

/** Exit codes are part of the CLI's contract; scripts branch on them. */
export const EXIT = {
  ok: 0,
  api: 1,
  usage: 2,
  auth: 3,
  notFound: 4,
  refused: 5,
  timeout: 6,
  unsafeStore: 7,
} as const;

export type ExitCode = (typeof EXIT)[keyof typeof EXIT];

/** An error the CLI raised itself, already classified. */
export class CliError extends Error {
  override readonly name = "CliError";
  readonly exitCode: ExitCode;
  readonly code: string;

  constructor(message: string, exitCode: ExitCode, code = "CLI_ERROR") {
    super(message);
    this.exitCode = exitCode;
    this.code = code;
  }
}

export function usageError(message: string): CliError {
  return new CliError(message, EXIT.usage, "CLI_USAGE");
}

export function authError(message: string): CliError {
  return new CliError(message, EXIT.auth, "CLI_AUTH");
}

/** Maps any failure to the exit code class a script can branch on. */
export function exitCodeFor(error: unknown): ExitCode {
  if (error instanceof CliError) return error.exitCode;
  if (error instanceof ManagementApiError) {
    if (error.status === 401 || error.status === 403) return EXIT.auth;
    if (error.status === 404) return EXIT.notFound;
    if (error.status === 409 || error.status === 412 || error.status === 422) return EXIT.refused;
    return EXIT.api;
  }
  return EXIT.api;
}

/** Renders an error for stderr: stable code, message, request id, retry advice. */
export function renderError(error: unknown): string {
  if (error instanceof ManagementApiError) {
    const parts = [`error ${error.code}: ${error.message}`];
    if (error.requestId) parts.push(`request ${error.requestId}`);
    const retry = error.retry as { readonly kind?: string; readonly afterMs?: number } | undefined;
    if (retry?.kind === "after_delay" && retry.afterMs !== undefined) {
      parts.push(`retry after ${Math.ceil(retry.afterMs / 1000)}s`);
    } else if (retry?.kind === "immediate") {
      parts.push("retryable");
    }
    return parts.join(" | ");
  }
  if (error instanceof CliError) return `error ${error.code}: ${error.message}`;
  if (error instanceof Error) return `error: ${error.message}`;
  return `error: ${String(error)}`;
}
