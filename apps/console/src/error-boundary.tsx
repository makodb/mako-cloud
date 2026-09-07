import {
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Eyebrow,
} from "@mako-cloud/ui";
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
      <main
        className="grid min-h-screen place-items-center bg-background p-6 text-foreground"
        aria-labelledby="console-error-title"
      >
        <Card role="alert" className="w-full max-w-xl">
          <CardHeader>
            <Eyebrow>Something went wrong</Eyebrow>
            <CardTitle as="h1" id="console-error-title" className="text-xl">
              The console could not load this view.
            </CardTitle>
            <CardDescription>{safeErrorMessage(this.state.error)}</CardDescription>
          </CardHeader>
          <CardContent className="grid gap-4">
            <RequestId value={requestId} />
            <div>
              <Button onClick={() => this.setState({ error: null })}>Try again</Button>
            </div>
          </CardContent>
        </Card>
      </main>
    );
  }
}

export function RequestId({ value }: { readonly value: string | null }) {
  if (value === null) {
    return null;
  }
  return (
    <p className="m-0 text-xs text-muted-foreground">
      Request ID: <code className="font-mono text-foreground break-all">{value}</code>
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
