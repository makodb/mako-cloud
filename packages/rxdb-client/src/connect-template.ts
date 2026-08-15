export const MAKO_RXDB_CONNECT_TEMPLATE_VERSION = 1 as const;

export interface MakoRxdbConnectTemplateV1Input {
  readonly endpoint: string;
  readonly projectId: string;
  readonly environmentId: string;
  readonly collectionId: string;
  readonly schemaVersion: number;
  readonly publicProjectKey: string;
}

/**
 * Emit the exact versioned quickstart shown by the developer console. The
 * template contains only public connection material and is compile-qualified
 * against this package and the supported RxDB major.
 */
export function createMakoRxdbConnectTemplateV1(input: MakoRxdbConnectTemplateV1Input): string {
  return `import { normalizeMakoRxdbConfig } from "@mako-cloud/rxdb";
import { RXDB_VERSION } from "rxdb/plugins/utils";

const config = normalizeMakoRxdbConfig({
  endpoint: ${JSON.stringify(input.endpoint)},
  projectId: ${JSON.stringify(input.projectId)},
  environmentId: ${JSON.stringify(input.environmentId)},
  collectionId: ${JSON.stringify(input.collectionId)},
  schemaVersion: ${input.schemaVersion},
  publicProjectKey: ${JSON.stringify(input.publicProjectKey)},
  rxdbVersion: RXDB_VERSION,
  runtime: "browser",
});`;
}
