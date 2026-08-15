import { Component, type ErrorInfo, type ReactNode } from "react";

import { ManagementApiError } from "@mako-cloud/management-sdk";

interface ErrorBoundaryState {
  readonly error: Error | null;
}

export class ConsoleErrorBoundary extends Component<
  { readonly children: ReactNode },
  ErrorBoundaryState
> {
  override state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: unknown): ErrorBoundaryState {
    return { error: error instanceof Error ? error : new Error("Unexpected console error") };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    globalThis.console.error("Mako console route failed", {
      name: error.name,
      requestId: requestIdFor(error),
      componentStack: info.componentStack,
    });
  }

  override render(): ReactNode {
    if (this.state.error === null) {
      return this.props.children;
    }
    const requestId = requestIdFor(this.state.error);
    return (
      <main className="centered" aria-labelledby="console-error-title">
        <section className="panel error-panel" role="alert">
          <p className="eyebrow">Something went wrong</p>
          <h1 id="console-error-title">The console could not load this view.</h1>
          <p>{safeErrorMessage(this.state.error)}</p>
          <RequestId value={requestId} />
          <button type="button" onClick={() => this.setState({ error: null })}>
            Try again
          </button>
        </section>
      </main>
    );
  }
}

export function RequestId({ value }: { readonly value: string | null }) {
  if (value === null) {
    return null;
  }
  return (
    <p className="request-id">
      Request ID: <code>{value}</code>
    </p>
  );
}

export function requestIdFor(error: unknown): string | null {
  return error instanceof ManagementApiError ? error.requestId : null;
}

function safeErrorMessage(error: Error): string {
  if (error instanceof ManagementApiError) {
    return error.message;
  }
  if (error.name === "DeveloperSessionExpiredError") {
    return error.message;
  }
  return "An unexpected error occurred. No sensitive diagnostics were displayed.";
}
