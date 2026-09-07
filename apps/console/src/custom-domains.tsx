import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  EmptyState,
  Field,
  Input,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  cn,
} from "@mako-cloud/ui";
import { Copy, Globe } from "lucide-react";
import { type FormEvent, type ReactNode, useCallback, useEffect, useState } from "react";

import type { CustomDomain, Environment } from "@mako-cloud/management-sdk";

import { type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { RequestId } from "./error-boundary.js";
import { useManagementClient } from "./management.js";
import { confirmDestructiveAction } from "./safety.js";

/** A lowercase fully qualified name: labels of letters, digits, and inner hyphens, two or more. */
const HOSTNAME =
  /^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/u;
const MAX_HOSTNAME_LENGTH = 253;

/** What the API's check outcomes mean for the person who has to fix them. */
const CHECK_OUTCOMES: Readonly<Record<string, string>> = {
  record_missing: "the TXT record was not found",
  record_mismatch: "the TXT record has a different value",
  dns_unavailable: "DNS could not be queried",
};

// A project's custom domains over the management API's domain routes: every
// hostname with the environment it serves and its verification state, the
// DNS record that proves control of it, a check on request, and removal.
// Nothing is served on a name before the platform has seen its record, and a
// name whose record later disappears stops being served and says so here.
export function CustomDomainsScreen({
  projectId,
  environments,
}: {
  readonly projectId: string;
  readonly environments: readonly Environment[];
}) {
  const client = useManagementClient();
  const [domains, setDomains] = useState<CustomDomain[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [shownRecord, setShownRecord] = useState<string | null>(null);
  const [acting, setActing] = useState<{ id: string; action: "verify" | "remove" } | null>(null);

  const reload = useCallback(async () => {
    try {
      setDomains(await client.listCustomDomains(projectId));
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  }, [client, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const add = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    setStatus(null);
    let hostname: string;
    try {
      hostname = parseHostname(String(data.get("hostname") ?? ""));
    } catch (error) {
      setFailure(failureFrom(error));
      return;
    }
    const environmentId = String(data.get("environmentId") ?? "");
    setAdding(true);
    try {
      const created = await client.createCustomDomain(
        projectId,
        { hostname, environmentId },
        idempotencyKey(),
      );
      setFailure(null);
      setStatus(
        `Domain ${created.hostname} added; publish its DNS record, then choose Verify now.`,
      );
      setShownRecord(created.id);
      form.reset();
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setAdding(false);
    }
  };

  const verify = async (domain: CustomDomain) => {
    setActing({ id: domain.id, action: "verify" });
    setStatus(null);
    try {
      const checked = await client.verifyCustomDomain(projectId, domain.id, idempotencyKey());
      setDomains((current) =>
        current === null
          ? current
          : current.map((item) => (item.id === checked.id ? checked : item)),
      );
      setFailure(null);
      setStatus(describeOutcome(checked));
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setActing(null);
    }
  };

  const remove = async (domain: CustomDomain) => {
    if (
      !confirmDestructiveAction({
        action: "Remove domain",
        target: domain.hostname,
        consequence:
          "Nothing is served on this name from then on and its certificate is no longer renewed. Add the domain again to serve it later.",
      })
    ) {
      return;
    }
    setActing({ id: domain.id, action: "remove" });
    setStatus(null);
    try {
      await client.deleteCustomDomain(projectId, domain.id, idempotencyKey());
      setFailure(null);
      setStatus(`Domain ${domain.hostname} removed; nothing is served on its name any more.`);
      if (shownRecord === domain.id) setShownRecord(null);
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setActing(null);
    }
  };

  const environmentName = (environmentId: string) =>
    environments.find((environment) => environment.id === environmentId)?.name ?? environmentId;
  const busy = acting !== null;
  const defaultEnvironment =
    environments.find((environment) => environment.state === "active") ?? environments[0];

  return (
    <div className="grid gap-4">
      <Card aria-labelledby="custom-domains-title">
        <CardHeader>
          <CardTitle id="custom-domains-title">Custom domains</CardTitle>
          <CardDescription>
            Serve an environment's API and functions on your own name with a certificate the
            platform obtains and renews. A domain is served only while its DNS verification record
            is in place.
          </CardDescription>
        </CardHeader>
        <CardContent className="grid gap-4">
          <FailureNotice failure={failure} />
          {status === null ? null : (
            <Alert variant="positive" role="status">
              <AlertDescription className="text-foreground">{status}</AlertDescription>
            </Alert>
          )}
          {domains === null ? (
            <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
              Loading domains…
            </p>
          ) : domains.length === 0 ? (
            <EmptyState
              icon={<Globe aria-hidden="true" />}
              title="No custom domains yet."
              description="Add one to serve an environment's API and functions on a name you own."
            />
          ) : (
            <div className="overflow-hidden rounded-lg border">
              <Table>
                <TableHeader>
                  <TableRow className="hover:bg-transparent">
                    <TableHead scope="col">Hostname</TableHead>
                    <TableHead scope="col">Environment</TableHead>
                    <TableHead scope="col">State</TableHead>
                    <TableHead scope="col">Verified</TableHead>
                    <TableHead scope="col">Last checked</TableHead>
                    <TableHead scope="col">
                      <span className="sr-only">Actions</span>
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {domains.map((domain) => {
                    const shown = shownRecord === domain.id;
                    return [
                      <TableRow
                        key={domain.id}
                        data-domain-id={domain.id}
                        data-state={domain.state}
                      >
                        <TableHead scope="row" className="align-top">
                          <code className="font-mono text-sm whitespace-nowrap">
                            {domain.hostname}
                          </code>
                          <small className="mt-1 block font-mono text-xs font-normal text-muted-foreground">
                            {domain.id}
                          </small>
                        </TableHead>
                        <TableCell className="align-top">
                          {environmentName(domain.environmentId)}
                        </TableCell>
                        <TableCell className="min-w-40 align-top whitespace-normal">
                          <DomainState domain={domain} />
                        </TableCell>
                        <TableCell className="align-top tabular-nums">
                          {domain.verifiedAt === null ? (
                            <em className="text-muted-foreground">never</em>
                          ) : (
                            <Timestamp value={domain.verifiedAt} />
                          )}
                        </TableCell>
                        <TableCell className="align-top tabular-nums">
                          {domain.lastCheckedAt === null ? (
                            <em className="text-muted-foreground">not yet</em>
                          ) : (
                            <Timestamp value={domain.lastCheckedAt} />
                          )}
                        </TableCell>
                        <TableCell className="align-top">
                          <div className="flex justify-end gap-1 whitespace-nowrap">
                            <Button
                              variant="outline"
                              size="sm"
                              aria-expanded={shown}
                              onClick={() => setShownRecord(shown ? null : domain.id)}
                            >
                              {shown ? "Hide DNS record" : "Show DNS record"}
                            </Button>
                            <Button
                              variant="outline"
                              size="sm"
                              disabled={busy}
                              onClick={() => void verify(domain)}
                            >
                              {acting?.id === domain.id && acting.action === "verify"
                                ? "Checking…"
                                : "Verify now"}
                            </Button>
                            <Button
                              variant="ghost"
                              size="sm"
                              className="text-destructive hover:bg-destructive/10 hover:text-destructive"
                              disabled={busy}
                              onClick={() => void remove(domain)}
                            >
                              {acting?.id === domain.id && acting.action === "remove"
                                ? "Removing…"
                                : "Remove"}
                            </Button>
                          </div>
                        </TableCell>
                      </TableRow>,
                      shown ? (
                        <TableRow
                          key={`${domain.id}-record`}
                          className="bg-muted/40 hover:bg-muted/40"
                        >
                          <TableCell colSpan={6} className="whitespace-normal p-4">
                            <DnsRecord domain={domain} />
                          </TableCell>
                        </TableRow>
                      ) : null,
                    ];
                  })}
                </TableBody>
              </Table>
            </div>
          )}
        </CardContent>
      </Card>
      <Card aria-labelledby="add-domain-title">
        <CardHeader>
          <CardTitle id="add-domain-title">Add a domain</CardTitle>
          <CardDescription>
            The name must be one you control. The answer carries a TXT record to create at your DNS
            provider; the platform checks it on request and on its own, and serves nothing on the
            name before it has seen the record.
          </CardDescription>
        </CardHeader>
        <CardContent>
          {environments.length === 0 ? (
            <p className="m-0 text-sm text-muted-foreground">
              Create an environment first; a domain serves exactly one.
            </p>
          ) : (
            <form
              className="grid items-end gap-4 sm:grid-cols-[minmax(0,2fr)_minmax(0,1fr)_auto]"
              onSubmit={(event) => void add(event)}
            >
              <Field label="Hostname" htmlFor="custom-domain-hostname">
                <Input
                  id="custom-domain-hostname"
                  name="hostname"
                  required
                  maxLength={MAX_HOSTNAME_LENGTH}
                  placeholder="api.example.com"
                  autoComplete="off"
                  spellCheck={false}
                  className="font-mono"
                />
              </Field>
              <Field label="Environment" htmlFor="custom-domain-environment">
                <NativeSelect
                  id="custom-domain-environment"
                  name="environmentId"
                  required
                  defaultValue={defaultEnvironment?.id ?? ""}
                >
                  {environments.map((environment) => (
                    <option key={environment.id} value={environment.id}>
                      {environment.name}
                      {environment.state === "active" ? "" : ` (${environment.state})`}
                    </option>
                  ))}
                </NativeSelect>
              </Field>
              <Button type="submit" disabled={adding}>
                {adding ? "Adding…" : "Add domain"}
              </Button>
            </form>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

/** The state as a badge; an unverified domain carries the last check's reason beside it. */
function DomainState({ domain }: { readonly domain: CustomDomain }) {
  const tone =
    domain.state === "verified" ? "success" : domain.state === "failed" ? "error" : "warning";
  const label =
    domain.state === "verified" ? "Verified" : domain.state === "failed" ? "Failed" : "Pending";
  return (
    <>
      <Badge
        className={cn("status-badge", tone)}
        variant={
          domain.state === "verified"
            ? "positive"
            : domain.state === "failed"
              ? "destructive"
              : "warning"
        }
      >
        {label}
      </Badge>
      {domain.state === "failed" ? (
        <small className="domain-error mt-1 block text-xs text-destructive">
          Serving stopped: {domain.lastError === null ? "re-verification failed" : reason(domain)}.
        </small>
      ) : domain.state === "pending" && domain.lastError !== null ? (
        <small className="domain-error mt-1 block text-xs text-destructive">
          Not verified: {reason(domain)}.
        </small>
      ) : null}
    </>
  );
}

/** The record to publish, each part copyable on its own. */
function DnsRecord({ domain }: { readonly domain: CustomDomain }) {
  const [copyStatus, setCopyStatus] = useState("");
  const copy = async (what: string, value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setCopyStatus(`Copied the record ${what}.`);
    } catch {
      setCopyStatus(`Clipboard access was unavailable; select the record ${what} and copy it.`);
    }
  };
  const { recordName, recordType, recordValue } = domain.verification;
  return (
    <section className="grid gap-3 text-sm" aria-label={`DNS record for ${domain.hostname}`}>
      <p className="m-0">
        Create this record at the DNS provider for <Code>{domain.hostname}</Code>, then choose
        Verify now. Keep it in place: the platform re-checks it and stops serving the domain if it
        goes missing.
      </p>
      <dl className="m-0 grid gap-x-6 gap-y-3 sm:grid-cols-[minmax(0,1fr)_auto_minmax(0,2fr)]">
        <div className="grid min-w-0 content-start gap-1.5">
          <dt className="text-xs font-medium text-muted-foreground">Name</dt>
          <dd className="m-0 grid justify-items-start gap-1.5">
            <code className="font-mono wrap-anywhere" data-field="recordName">
              {recordName}
            </code>
            <Button variant="outline" size="sm" onClick={() => void copy("name", recordName)}>
              <Copy aria-hidden="true" className="size-3.5" />
              Copy name
            </Button>
          </dd>
        </div>
        <div className="grid min-w-0 content-start gap-1.5">
          <dt className="text-xs font-medium text-muted-foreground">Type</dt>
          <dd className="m-0">
            <code className="font-mono" data-field="recordType">
              {recordType}
            </code>
          </dd>
        </div>
        <div className="grid min-w-0 content-start gap-1.5">
          <dt className="text-xs font-medium text-muted-foreground">Value</dt>
          <dd className="m-0 grid justify-items-start gap-1.5">
            <code className="font-mono wrap-anywhere" data-field="recordValue">
              {recordValue}
            </code>
            <Button variant="outline" size="sm" onClick={() => void copy("value", recordValue)}>
              <Copy aria-hidden="true" className="size-3.5" />
              Copy value
            </Button>
          </dd>
        </div>
      </dl>
      {copyStatus === "" ? null : (
        <p className="m-0 text-xs text-muted-foreground" aria-live="polite" aria-atomic="true">
          {copyStatus}
        </p>
      )}
    </section>
  );
}

function Timestamp({ value }: { readonly value: string }) {
  return (
    <time dateTime={value} className="whitespace-nowrap">
      {new Date(value).toLocaleString()}
    </time>
  );
}

/** An identifier or a hostname inside a sentence: monospace on a quiet chip. */
function Code({ children }: { readonly children: ReactNode }) {
  return <code className="rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]">{children}</code>;
}

/** A failed request, with its request id when the API gave one. */
function FailureNotice({ failure }: { readonly failure: ConsoleApiFailure | null }) {
  if (failure === null) {
    return null;
  }
  return (
    <Alert variant="destructive" role="alert">
      <AlertDescription>
        <p className="m-0">{failure.message}</p>
        <RequestId value={failure.requestId} />
      </AlertDescription>
    </Alert>
  );
}

/** The API's outcome code with its meaning when the console knows it. */
function reason(domain: CustomDomain): string {
  const code = domain.lastError ?? "";
  const meaning = CHECK_OUTCOMES[code];
  return meaning === undefined ? code : `${meaning} (${code})`;
}

function describeOutcome(domain: CustomDomain): string {
  if (domain.state === "verified") {
    return `Domain ${domain.hostname} verified; it is served with a managed certificate.`;
  }
  const detail = domain.lastError === null ? "" : `: ${reason(domain)}`;
  return domain.state === "failed"
    ? `Domain ${domain.hostname} failed verification${detail}; serving stopped until it verifies again.`
    : `Domain ${domain.hostname} is not verified yet${detail}. Publish the record and check again.`;
}

function parseHostname(raw: string): string {
  const hostname = raw.trim().toLowerCase().replace(/\.$/u, "");
  if (hostname.length > MAX_HOSTNAME_LENGTH || !HOSTNAME.test(hostname)) {
    throw new FormInputError(
      "Hostname must be a fully qualified DNS name such as api.example.com: lowercase labels of letters, digits, and hyphens, separated by dots.",
    );
  }
  return hostname;
}

class FormInputError extends Error {}

function failureFrom(error: unknown): ConsoleApiFailure {
  return error instanceof FormInputError
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}
