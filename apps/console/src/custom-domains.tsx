import { type FormEvent, useCallback, useEffect, useState } from "react";

import type { CustomDomain, Environment } from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
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
    <>
      <section
        className="panel full-span project-home-panel custom-domains"
        aria-labelledby="custom-domains-title"
      >
        <div className="section-heading">
          <div>
            <h2 id="custom-domains-title">Custom domains</h2>
            <p>
              Serve an environment's API and functions on your own name with a certificate the
              platform obtains and renews. A domain is served only while its DNS verification record
              is in place.
            </p>
          </div>
        </div>
        <ApiFailureNotice failure={failure} />
        {status === null ? null : (
          <p className="notice success" role="status">
            {status}
          </p>
        )}
        {domains === null ? (
          <p aria-live="polite">Loading domains…</p>
        ) : domains.length === 0 ? (
          <p>
            No custom domains yet. Add one to serve an environment's API and functions on a name you
            own.
          </p>
        ) : (
          <div className="table-scroll">
            <table className="domain-table">
              <thead>
                <tr>
                  <th scope="col">Hostname</th>
                  <th scope="col">Environment</th>
                  <th scope="col">State</th>
                  <th scope="col">Verified</th>
                  <th scope="col">Last checked</th>
                  <th scope="col">
                    <span className="visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {domains.map((domain) => {
                  const shown = shownRecord === domain.id;
                  return [
                    <tr key={domain.id} data-domain-id={domain.id} data-state={domain.state}>
                      <th scope="row">
                        <code className="domain-hostname">{domain.hostname}</code>
                        <small className="domain-id">{domain.id}</small>
                      </th>
                      <td>{environmentName(domain.environmentId)}</td>
                      <td>
                        <DomainState domain={domain} />
                      </td>
                      <td>
                        {domain.verifiedAt === null ? (
                          <em>never</em>
                        ) : (
                          <Timestamp value={domain.verifiedAt} />
                        )}
                      </td>
                      <td>
                        {domain.lastCheckedAt === null ? (
                          <em>not yet</em>
                        ) : (
                          <Timestamp value={domain.lastCheckedAt} />
                        )}
                      </td>
                      <td>
                        <div className="button-row">
                          <button
                            type="button"
                            className="secondary"
                            aria-expanded={shown}
                            onClick={() => setShownRecord(shown ? null : domain.id)}
                          >
                            {shown ? "Hide DNS record" : "Show DNS record"}
                          </button>
                          <button
                            type="button"
                            className="secondary"
                            disabled={busy}
                            onClick={() => void verify(domain)}
                          >
                            {acting?.id === domain.id && acting.action === "verify"
                              ? "Checking…"
                              : "Verify now"}
                          </button>
                          <button
                            type="button"
                            className="danger-link"
                            disabled={busy}
                            onClick={() => void remove(domain)}
                          >
                            {acting?.id === domain.id && acting.action === "remove"
                              ? "Removing…"
                              : "Remove"}
                          </button>
                        </div>
                      </td>
                    </tr>,
                    shown ? (
                      <tr key={`${domain.id}-record`} className="domain-record-row">
                        <td colSpan={6}>
                          <DnsRecord domain={domain} />
                        </td>
                      </tr>
                    ) : null,
                  ];
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>
      <section
        className="panel full-span project-home-panel custom-domains"
        aria-labelledby="add-domain-title"
      >
        <h2 id="add-domain-title">Add a domain</h2>
        <p>
          The name must be one you control. The answer carries a TXT record to create at your DNS
          provider; the platform checks it on request and on its own, and serves nothing on the name
          before it has seen the record.
        </p>
        {environments.length === 0 ? (
          <p>Create an environment first; a domain serves exactly one.</p>
        ) : (
          <form
            className="inline-form project-home-form domain-form"
            onSubmit={(event) => void add(event)}
          >
            <label>
              Hostname
              <input
                name="hostname"
                required
                maxLength={MAX_HOSTNAME_LENGTH}
                placeholder="api.example.com"
                autoComplete="off"
                spellCheck={false}
              />
            </label>
            <label>
              Environment
              <select name="environmentId" required defaultValue={defaultEnvironment?.id ?? ""}>
                {environments.map((environment) => (
                  <option key={environment.id} value={environment.id}>
                    {environment.name}
                    {environment.state === "active" ? "" : ` (${environment.state})`}
                  </option>
                ))}
              </select>
            </label>
            <button type="submit" disabled={adding}>
              {adding ? "Adding…" : "Add domain"}
            </button>
          </form>
        )}
      </section>
    </>
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
      <span className={`status-badge ${tone}`}>{label}</span>
      {domain.state === "failed" ? (
        <small className="domain-error">
          Serving stopped: {domain.lastError === null ? "re-verification failed" : reason(domain)}.
        </small>
      ) : domain.state === "pending" && domain.lastError !== null ? (
        <small className="domain-error">Not verified: {reason(domain)}.</small>
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
    <section className="domain-record" aria-label={`DNS record for ${domain.hostname}`}>
      <p>
        Create this record at the DNS provider for <code>{domain.hostname}</code>, then choose
        Verify now. Keep it in place: the platform re-checks it and stops serving the domain if it
        goes missing.
      </p>
      <dl className="definition-grid">
        <div>
          <dt>Name</dt>
          <dd>
            <code data-field="recordName">{recordName}</code>
            <button
              type="button"
              className="secondary"
              onClick={() => void copy("name", recordName)}
            >
              Copy name
            </button>
          </dd>
        </div>
        <div>
          <dt>Type</dt>
          <dd>
            <code data-field="recordType">{recordType}</code>
          </dd>
        </div>
        <div>
          <dt>Value</dt>
          <dd>
            <code data-field="recordValue">{recordValue}</code>
            <button
              type="button"
              className="secondary"
              onClick={() => void copy("value", recordValue)}
            >
              Copy value
            </button>
          </dd>
        </div>
      </dl>
      {copyStatus === "" ? null : (
        <p className="copy-status" aria-live="polite" aria-atomic="true">
          {copyStatus}
        </p>
      )}
    </section>
  );
}

function Timestamp({ value }: { readonly value: string }) {
  return <time dateTime={value}>{new Date(value).toLocaleString()}</time>;
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
