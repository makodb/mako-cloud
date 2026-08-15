import createClient from "openapi-fetch";
import type { ClientOptions } from "openapi-fetch";

import type { paths } from "./generated/schema.js";

export type MakoApiClient = ReturnType<typeof createClient<paths>>;

/** Creates an isolated typed client for one Mako Cloud API origin. */
export function createMakoApiClient(options: ClientOptions): MakoApiClient {
  return createClient<paths>(options);
}
