/**
 * What the application exposes to its browser tests. Both the wire-mocked
 * suite and the live suite drive the same `window.rational` object.
 */
interface RationalDocumentWire {
  id: string;
  household_id: string;
  created_at: number;
  updated_at: number;
  [field: string]: unknown;
}

interface RationalScopeStateWire {
  generation: number;
  initialSynced: boolean;
  activity: string;
  pendingWrites: number;
  received: number;
  sent: number;
  conflicts: number;
  errors: number;
  securityResets: number;
  fullResyncs: number;
  streamResyncs: number;
  syncedAt: number | null;
  notice: string | null;
}

interface RationalDiagnosticsWire {
  phase: string;
  connectivity: string;
  online: boolean;
  currentHouseholdId: string | null;
  memberships: number;
  activity: string;
  pendingWrites: number;
  received: number;
  sent: number;
  conflicts: number;
  errors: number;
  lastError: string | null;
  securityResets: number;
  fullResyncs: number;
  streamResyncs: number;
  authRequests: number;
  refreshes: number;
  pullRequests: number;
  pushRequests: number;
  acceptedWrites: number;
  conflictResponses: number;
  deniedWrites: number;
  streamConnections: number;
  failedRequests: number;
  household: RationalScopeStateWire | null;
  directory: RationalScopeStateWire | null;
}

interface RationalWritesWire {
  createTransaction(input: Record<string, unknown>): Promise<RationalDocumentWire>;
  updateTransaction(
    id: string,
    patch: Record<string, unknown>,
    updatedAt?: number,
  ): Promise<RationalDocumentWire>;
  deleteTransaction(id: string): Promise<void>;
  createAccount(input: Record<string, unknown>): Promise<RationalDocumentWire>;
  createCategory(name: string, kind: string): Promise<RationalDocumentWire>;
  createRule(input: Record<string, unknown>): Promise<RationalDocumentWire>;
  updateRule(id: string, patch: Record<string, unknown>): Promise<RationalDocumentWire>;
  importTransactions(input: Record<string, unknown>): Promise<Record<string, unknown>>;
  setBudget(input: Record<string, unknown>): Promise<RationalDocumentWire>;
  saveRecurrence(input: Record<string, unknown>): Promise<RationalDocumentWire>;
  updateRecurrence(id: string, patch: Record<string, unknown>): Promise<RationalDocumentWire>;
  saveAlertSetting(input: Record<string, unknown>): Promise<RationalDocumentWire>;
  markAlertRead(id: string, read?: boolean): Promise<RationalDocumentWire>;
  createGoal(input: Record<string, unknown>): Promise<RationalDocumentWire>;
  contributeToGoal(
    id: string,
    contribution: Record<string, unknown>,
  ): Promise<RationalDocumentWire>;
}

/** The household's receipts, as a browser test reaches them. */
interface RationalReceiptsWire {
  list(
    transactionId: string,
  ): Promise<ReadonlyArray<{ path: string; name: string; contentType: string; sizeBytes: number }>>;
  open(path: string): Promise<Blob | null>;
  remove(path: string): Promise<void>;
}

interface RationalCollectionWire {
  find(query?: Record<string, unknown>): {
    exec(): Promise<Array<{ toJSON(): RationalDocumentWire }>>;
  };
  findOne(id: string): { exec(): Promise<{ toJSON(): RationalDocumentWire } | null> };
}

interface RationalHouseholdWire {
  session: {
    identifier: string;
    collections: Record<string, RationalCollectionWire>;
  } | null;
}

interface RationalBrowserApplication {
  state: {
    phase: string;
    currentHouseholdId: string | null;
    memberships: Array<{ household_id: string; role: string }>;
    invitations: Array<{ household_id: string; role: string; email?: string }>;
    connectivity: string;
    authError: string | null;
    magicLinkSentTo: string | null;
    user: { id: string; email: string } | null;
    households: Array<{ id: string; name: string }>;
    generation: number;
    directory: RationalScopeStateWire | null;
    household: RationalScopeStateWire | null;
  };
  household: RationalHouseholdWire | null;
  directory: RationalHouseholdWire | null;
  writes: RationalWritesWire | null;
  receipts: RationalReceiptsWire | null;
  signIn(email: string, password: string): Promise<void>;
  signUp(email: string, password: string): Promise<void>;
  signOut(): Promise<void>;
  selectHousehold(id: string | null): Promise<void>;
  setOnline(online: boolean): Promise<void>;
  online(): boolean;
  waitForSync(): Promise<void>;
  disconnectStreams(): void;
  refreshSession(): Promise<void>;
  forceSecurityReset(): Promise<void>;
  createHousehold(name: string, currency: string): Promise<string>;
  inviteMember(householdId: string, email: string, role: string): Promise<void>;
  acceptInvitation(householdId: string): Promise<void>;
  changeMemberRole(householdId: string, userId: string, role: string): Promise<void>;
  removeMember(householdId: string, userId: string): Promise<void>;
  roleIn(householdId: string | null): string | null;
  diagnostics(): RationalDiagnosticsWire;
}

interface RationalFakeBackend {
  demoHouseholdId: string;
  putRemote(collectionId: string, document: RationalDocumentWire): void;
  deleteRemote(collectionId: string, id: string, updatedAt: number): void;
  remoteDocument(collectionId: string, id: string): RationalDocumentWire | undefined;
  setRole(email: string, householdId: string, role: string | null): void;
  roleOf(email: string, householdId: string): string | null;
  lastMagicLink(): string | null;
  revokeAccess(email: string): void;
  disconnectStreams(): void;
  advanceClock(milliseconds: number): void;
}

interface Window {
  rational: RationalBrowserApplication;
  rationalFake?: RationalFakeBackend;
  __RATIONAL__?: Record<string, unknown>;
}
