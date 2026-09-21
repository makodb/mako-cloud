/**
 * Globals the reference application exposes to its browser tests. Both the
 * fake-backed suite and the live suite drive the same application object, so
 * the shape is declared once here rather than in each spec.
 */
interface ReferenceTodoWire {
  id: string;
  ownerId: string;
  title: string;
  updatedAt: number;
}

interface ReferenceDiagnosticsWire {
  acceptedWrites: number;
  activity: string;
  conflicts: number;
  reconnects: number;
  refreshes: number;
  streamConnections: number;
}

interface ReferenceBrowserApplication {
  addTodo(document: ReferenceTodoWire): Promise<void>;
  deleteTodo(id: string): Promise<void>;
  diagnostics(): ReferenceDiagnosticsWire;
  forceReconnect(): Promise<void>;
  forceTokenRefresh(): Promise<void>;
  listTodos(): Promise<ReferenceTodoWire[]>;
  putRemote(document: ReferenceTodoWire): Promise<void>;
  removeRemote(id: string, updatedAt: number): Promise<void>;
  revokeAccess(): Promise<void>;
  setOnline(online: boolean): Promise<void>;
  updateTodo(id: string, title: string, updatedAt: number): Promise<void>;
  waitForSync(): Promise<void>;
}

interface Window {
  makoExample: ReferenceBrowserApplication;
  /** Present only when the app is pointed at a real deployment. */
  __MAKO_EXAMPLE__?: Record<string, unknown>;
}
