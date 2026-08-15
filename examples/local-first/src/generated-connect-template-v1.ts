// Compile qualification fixture for the exact version-1 console template.
import { normalizeMakoRxdbConfig } from "@mako-cloud/rxdb";
import { RXDB_VERSION } from "rxdb/plugins/utils";

export const generatedTemplateConfig = normalizeMakoRxdbConfig({
  endpoint: "https://cloud-test.makodb.com",
  projectId: "prj_example001",
  environmentId: "env_example001",
  collectionId: "todos",
  schemaVersion: 1,
  publicProjectKey: "mako_pk.PASTE_ONE_TIME_PUBLIC_KEY",
  rxdbVersion: RXDB_VERSION,
  runtime: "browser",
});
