/** Reference local-first application entrypoint. */
export type {
  ReferenceBackend,
  ReferenceBackendConfig,
  ReferenceBackendDiagnostics,
} from "./backend.js";
export { type LiveBackendOptions, LiveMakoBackend } from "./live-backend.js";
export { type FakeBackendDiagnostics, FakeMakoBackend } from "./mock-backend.js";
export {
  createReferenceApplication,
  type ReferenceApplication,
  type ReferenceApplicationDiagnostics,
  type ReferenceTodo,
} from "./reference-app.js";

export const EXAMPLE_NAME = "mako-local-first" as const;
