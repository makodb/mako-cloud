/** Reference local-first application entrypoint. */
export { FakeMakoBackend, type FakeBackendDiagnostics } from "./mock-backend.js";
export {
  createReferenceApplication,
  type ReferenceApplication,
  type ReferenceApplicationDiagnostics,
  type ReferenceTodo,
} from "./reference-app.js";

export const EXAMPLE_NAME = "mako-local-first" as const;
