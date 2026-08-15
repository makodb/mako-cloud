import assert from "node:assert/strict";
import test from "node:test";
import { BehaviorSubject, Subject } from "rxjs";

import { MakoReplicationError, MakoReplicationSignals } from "../dist/node/index.js";

test("exposes activity, documents, conflicts, throttling, resets, and sanitized errors", () => {
  const active$ = new BehaviorSubject(false);
  const received$ = new Subject();
  const sent$ = new Subject();
  const conflict$ = new Subject();
  const error$ = new Subject();
  const signals = new MakoReplicationSignals();
  const observed = { activity: [], received: [], sent: [], conflicts: [], throttle: [], errors: [] };
  signals.activity$.subscribe((value) => observed.activity.push(value));
  signals.received$.subscribe((value) => observed.received.push(value));
  signals.sent$.subscribe((value) => observed.sent.push(value));
  signals.conflicts$.subscribe((value) => observed.conflicts.push(value));
  signals.throttling$.subscribe((value) => observed.throttle.push(value));
  signals.errors$.subscribe((value) => observed.errors.push(value));
  const binding = signals.bind({ active$, received$, sent$, conflict$, error$ });
  active$.next(true);
  received$.next({ id: "received" });
  sent$.next({ id: "sent" });
  conflict$.next({ documentId: "todo-1" });
  error$.next(
    new MakoReplicationError({
      code: "rate_limited",
      message: "slow down",
      requestId: "req_rate",
      retry: { kind: "after_delay", afterMs: 1500 },
    }),
  );
  error$.next(new Error("protected raw error content"));
  assert.equal(observed.activity.at(-1), "active");
  assert.equal(observed.received[0].id, "received");
  assert.equal(observed.sent[0].id, "sent");
  assert.equal(observed.conflicts[0].documentId, "todo-1");
  assert.deepEqual(observed.throttle[0], {
    code: "rate_limited",
    retryAfterMilliseconds: 1500,
  });
  assert.equal(observed.errors[1].message, "replication operation failed");
  assert.equal(JSON.stringify(observed.errors).includes("protected raw error content"), false);
  binding.unsubscribe();
});
