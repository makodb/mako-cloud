import { createInterface } from "node:readline";
import { Writable } from "node:stream";

import { authError, usageError } from "./errors.js";

export interface PromptIo {
  readonly stdin: NodeJS.ReadableStream | null;
  readonly stderr: { write(chunk: string): unknown };
  readonly isTTY: boolean;
}

function readLine(io: PromptIo, question: string, echo: boolean): Promise<string> {
  const input = io.stdin;
  if (input === null) {
    return Promise.reject(usageError("a terminal is required to answer this prompt"));
  }
  const sink = new Writable({
    write(_chunk, _encoding, callback) {
      callback();
    },
  });
  const rl = createInterface({ input, output: echo ? undefined : sink, terminal: io.isTTY });
  io.stderr.write(question);
  return new Promise((resolve, reject) => {
    rl.once("close", () => reject(usageError("input closed before the prompt was answered")));
    rl.question("", (answer) => {
      rl.removeAllListeners("close");
      rl.close();
      if (!echo) io.stderr.write("\n");
      resolve(answer);
    });
  });
}

/** Asks a question on the terminal and returns the typed line. */
export function promptLine(io: PromptIo, question: string): Promise<string> {
  if (!io.isTTY) return Promise.reject(usageError(`${question.trim()} — no terminal to ask on`));
  return readLine(io, question, true);
}

/** Asks for a secret with echo off; refuses without a terminal. */
export function promptSecret(io: PromptIo, question: string): Promise<string> {
  if (!io.isTTY) {
    return Promise.reject(
      authError(
        `${question.trim()} — a terminal is required; passwords are never taken from arguments`,
      ),
    );
  }
  return readLine(io, question, false);
}
