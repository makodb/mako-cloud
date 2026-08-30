import assert from "node:assert/strict";
import test from "node:test";

import {
  SUPPORTED_RXDB_RANGE,
  UnsupportedRxdbVersionError,
  assertSupportedRxdbVersion,
  normalizeMakoRxdbConfig,
} from "../dist/node/index.js";

test("accepts the supported RxDB major and rejects adjacent majors", () => {
  assert.doesNotThrow(() => assertSupportedRxdbVersion("17.4.0"));
  for (const version of ["16.9.0", "18.0.0", "17.0.0-beta.1", "unknown"]) {
    assert.throws(
      () => assertSupportedRxdbVersion(version),
      (error) =>
        error instanceof UnsupportedRxdbVersionError &&
        error.supportedRange === SUPPORTED_RXDB_RANGE,
    );
  }
});

test("normalizes a bounded browser or Node configuration", () => {
  const config = normalizeMakoRxdbConfig({
    endpoint: "http://localhost:8787/",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    collectionId: "todos",
    schemaVersion: 1,
    publicProjectKey: "mako_pk.public.example",
    rxdbVersion: "17.4.0",
    runtime: "node",
  });
  assert.equal(config.endpoint.toString(), "http://localhost:8787/");
  assert.equal(config.pullBatchSize, 100);
  assert.equal(config.runtime, "node");
});

test("a replication filter is bounded and defaults to none", () => {
  const base = {
    endpoint: "http://localhost:8787/",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    collectionId: "transactions",
    schemaVersion: 1,
    publicProjectKey: "mako_pk.public.example",
    rxdbVersion: "17.4.0",
    runtime: "node",
  };
  assert.equal(normalizeMakoRxdbConfig(base).filter, null);
  assert.deepEqual(
    normalizeMakoRxdbConfig({ ...base, filter: { field: "household_id", value: "hh_one" } }).filter,
    { field: "household_id", value: "hh_one" },
  );
  // A field the platform would refuse is refused here, where the message can
  // name the value: the alternative is a replication that silently matches
  // nothing.
  for (const filter of [
    { field: "", value: "hh_one" },
    { field: "1household", value: "hh_one" },
    { field: "household-id", value: "hh_one" },
    { field: "household_id", value: "a".repeat(257) },
    { field: "household_id", value: "line\nbreak" },
  ]) {
    assert.throws(() => normalizeMakoRxdbConfig({ ...base, filter }), /filter\.(field|value)/u);
  }
});
