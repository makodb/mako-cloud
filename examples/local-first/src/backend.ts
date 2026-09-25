import type { AuthSessionPersistence, MakoAuthClient } from "@mako-cloud/rxdb";

import type { ReferenceTodo } from "./reference-app.js";

/** Counters the reference application surfaces as diagnostics. */
export interface ReferenceBackendDiagnostics {
  readonly acceptedWrites: number;
  readonly conflictResponses: number;
  readonly online: boolean;
  readonly refreshes: number;
  readonly streamConnections: number;
}

/** Connection details the reference application needs to reach a backend. */
export interface ReferenceBackendConfig {
  readonly endpoint: string;
  readonly projectId: string;
  readonly environmentId: string;
  readonly collectionId: string;
  readonly publicProjectKey: string;
}

/**
 * The seam between the reference application and whatever is answering its
 * requests. `FakeMakoBackend` implements it in the browser so the app is
 * runnable and its tests are hermetic; `LiveMakoBackend` implements it against
 * a running Mako deployment so the same scenarios prove the server side too.
 *
 * Both must behave identically from the application's point of view. Where they
 * cannot — a real access token is not revoked by mutating a field — the
 * operation is expressed as intent here and each backend achieves it its own
 * way.
 */
export interface ReferenceBackend {
  readonly config: ReferenceBackendConfig;
  readonly fetch: typeof globalThis.fetch;
  readonly now: () => number;
  /**
   * Keep the local database in the browser (IndexedDB) rather than in memory,
   * so writes made offline survive the tab closing until they are pushed.
   */
  readonly persistLocalData?: boolean;
  /** Where the session is kept; left out, it lives only as long as the page. */
  readonly sessionPersistence?: AuthSessionPersistence | undefined;
  /** Establish the application's session, registering the user if required. */
  authenticate(auth: MakoAuthClient): Promise<void>;
  diagnostics(): ReferenceBackendDiagnostics;
  /** Simulate loss and restoration of connectivity. */
  setOnline(online: boolean): void;
  /** Write a document as some other client would. */
  putRemote(document: ReferenceTodo): Promise<void>;
  /** Delete a document as some other client would. */
  deleteRemote(id: string, updatedAt: number): Promise<void>;
  /** Force the access token to be exchanged for a new one. */
  forceTokenRefresh(auth: MakoAuthClient): Promise<void>;
  /** Break every open live stream so the client must reconnect. */
  disconnectStreams(): void;
  /** Make the current session permanently unusable. */
  revokeAccess(auth: MakoAuthClient): Promise<void>;
  /**
   * Test hook: answer every replication request as a server whose collection
   * has moved on to `version`. Only the in-browser fake can do this.
   */
  requireSchemaVersion?(version: number): void;
}
