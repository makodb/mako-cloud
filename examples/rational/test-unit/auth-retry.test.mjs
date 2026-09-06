import assert from "node:assert/strict";
import { test } from "node:test";

import { MakoAuthError, MakoAuthenticationRequiredError } from "@mako-cloud/rxdb";

import { throughTransient } from "../src/auth.ts";

/**
 * The hosted platform restarts a service for a couple of seconds when it
 * takes a checkpoint backup, and an auth `fetch` caught in that window throws
 * a retryable `MakoAuthError` ("authentication service is unavailable"). A
 * person clicking Sign in should ride through that, not see a wall -- but a
 * real refusal must fail at once. This guards the seam that used to check the
 * wrong subclass, so the retry never fired.
 */

test("a transient unavailability is retried until it succeeds", async () => {
  let calls = 0;
  const result = await throughTransient(() => {
    calls += 1;
    if (calls < 3) {
      // The exact shape the client throws when a fetch fails mid-backup.
      throw new MakoAuthError("authentication service is unavailable", {
        code: "unavailable",
        retryable: true,
      });
    }
    return Promise.resolve("signed in");
  });
  assert.equal(result, "signed in");
  assert.equal(calls, 3, "it kept trying through the outage");
});

test("a definitive refusal is not retried", async () => {
  let calls = 0;
  await assert.rejects(
    throughTransient(() => {
      calls += 1;
      throw new MakoAuthenticationRequiredError("email or password do not match", {
        code: "unauthenticated",
        status: 401,
        retryable: false,
      });
    }),
    /do not match/,
  );
  assert.equal(calls, 1, "a wrong password is surfaced at once, not retried");
});

test("a persistent outage gives up within a bounded budget rather than hanging", async () => {
  let calls = 0;
  const started = Date.now();
  await assert.rejects(
    throughTransient(() => {
      calls += 1;
      throw new MakoAuthError("authentication service is unavailable", { retryable: true });
    }),
    /unavailable/,
  );
  // Four attempts (one plus three backoffs of 400 + 900 + 1600 ms): bounded,
  // and shorter than a backup window, so a genuine outage fails promptly.
  assert.equal(calls, 4);
  assert.ok(Date.now() - started < 5_000, "the whole budget stays under five seconds");
});
