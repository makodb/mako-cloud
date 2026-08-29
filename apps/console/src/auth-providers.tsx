import { type FormEvent, useCallback, useEffect, useState } from "react";

import type {
  AuthProviderKind,
  AuthProviderView,
  AuthSettings,
  AuthSettingsUpdate,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";

const PROVIDER_NAME_PATTERN = "[a-z][a-z0-9-]{1,63}";
const DEFAULT_LINK_TTL_SECONDS = 900;

/** A provider as the developer is editing it: the installed view plus a
 * secret typed this session, which is sent once and never shown again. */
interface ProviderDraft {
  readonly name: string;
  readonly kind: AuthProviderKind;
  readonly clientId: string;
  readonly scopes: readonly string[];
  readonly enabled: boolean;
  readonly hasSecret: boolean;
  readonly newSecret: string | null;
}

// Sign-in settings for one environment: external providers, the redirect
// allowlist, and magic links. Edits are held locally and saved together, as
// the API installs the settings as one unit. A client secret is written
// once and never read back; the screen only ever says whether one is stored.
export function AuthProvidersScreen({
  projectId,
  environmentId,
}: {
  readonly projectId: string;
  readonly environmentId: string;
}) {
  const client = useManagementClient();
  const [installed, setInstalled] = useState<AuthSettings | null>(null);
  const [providers, setProviders] = useState<ProviderDraft[]>([]);
  const [redirectUrls, setRedirectUrls] = useState("");
  const [magicLinks, setMagicLinks] = useState({
    enabled: false,
    linkTtlSeconds: DEFAULT_LINK_TTL_SECONDS,
  });
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [dirty, setDirty] = useState(false);

  const adopt = useCallback((settings: AuthSettings) => {
    setInstalled(settings);
    setProviders(settings.providers.map(draftFrom));
    setRedirectUrls(settings.redirectUrls.join("\n"));
    setMagicLinks({
      enabled: settings.magicLinks.enabled,
      linkTtlSeconds: settings.magicLinks.linkTtlSeconds,
    });
    setDirty(false);
  }, []);
  const reload = useCallback(async () => {
    try {
      adopt(await client.getAuthSettings(projectId, environmentId));
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  }, [adopt, client, environmentId, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const edit = (update: (current: ProviderDraft[]) => ProviderDraft[]) => {
    setProviders(update);
    setDirty(true);
    setStatus(null);
  };

  const addProvider = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    const name = requiredText(data, "name");
    if (providers.some((provider) => provider.name === name)) {
      setFailure({
        message: `A provider named ${name} is already configured; edit it in the list instead.`,
        requestId: null,
      });
      return;
    }
    const kindType = requiredText(data, "kind");
    const kind: AuthProviderKind =
      kindType === "git_hub"
        ? { type: "git_hub" }
        : { type: "oidc", issuer: requiredText(data, "issuer") };
    const secret = String(data.get("clientSecret") ?? "").trim();
    edit((current) => [
      ...current,
      {
        name,
        kind,
        clientId: requiredText(data, "clientId"),
        scopes: scopesFrom(String(data.get("scopes") ?? "")),
        enabled: data.get("enabled") === "on",
        hasSecret: false,
        newSecret: secret === "" ? null : secret,
      },
    ]);
    setFailure(null);
    form.reset();
  };

  const save = async () => {
    const missing = providers.filter(
      (provider) => !provider.hasSecret && provider.newSecret === null,
    );
    if (missing.length > 0) {
      setFailure({
        message: `Enter the client secret for ${missing.map((provider) => provider.name).join(", ")} before saving.`,
        requestId: null,
      });
      return;
    }
    setSaving(true);
    setStatus(null);
    try {
      const input: AuthSettingsUpdate = {
        providers: providers.map((provider) => ({
          name: provider.name,
          kind: provider.kind,
          clientId: provider.clientId,
          scopes: [...provider.scopes],
          enabled: provider.enabled,
          ...(provider.newSecret === null ? {} : { clientSecret: provider.newSecret }),
        })),
        redirectUrls: redirectUrls
          .split(/\r?\n/u)
          .map((line) => line.trim())
          .filter((line) => line !== ""),
        magicLinks: {
          enabled: magicLinks.enabled,
          linkTtlSeconds: magicLinks.linkTtlSeconds,
        },
      };
      const saved = await client.updateAuthSettings(
        projectId,
        environmentId,
        input,
        idempotencyKey(),
      );
      adopt(saved);
      setFailure(null);
      setStatus(`Sign-in settings saved (version ${saved.version}).`);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section aria-labelledby="auth-providers-title" className="auth-providers-screen">
      <div className="section-heading">
        <div>
          <p className="eyebrow">Environment {environmentId}</p>
          <h1 id="auth-providers-title">Auth providers</h1>
          <p>
            How your application's users sign in besides email and password: external OpenID Connect
            and GitHub providers, the pages the sign-in flow may return to, and magic links. Client
            secrets are sealed on save and never shown again.
          </p>
        </div>
        {installed === null ? null : <p className="muted">Installed version {installed.version}</p>}
      </div>
      {failure === null ? null : <ApiFailureNotice failure={failure} />}
      {status === null ? null : (
        <p role="status" className="notice success">
          {status}
        </p>
      )}
      {installed === null && failure === null ? (
        <p role="status">Loading sign-in settings…</p>
      ) : null}

      <h2 id="providers-heading">Sign-in providers</h2>
      {providers.length === 0 ? (
        <p className="muted">No external providers yet. Add one below.</p>
      ) : (
        <table aria-labelledby="providers-heading" className="data-table">
          <thead>
            <tr>
              <th scope="col">Provider</th>
              <th scope="col">Kind</th>
              <th scope="col">Client ID</th>
              <th scope="col">Scopes</th>
              <th scope="col">Secret</th>
              <th scope="col">Enabled</th>
              <th scope="col">
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {providers.map((provider) => (
              <tr key={provider.name}>
                <th scope="row">
                  <code>{provider.name}</code>
                </th>
                <td>{describeKind(provider.kind)}</td>
                <td>
                  <code>{provider.clientId}</code>
                </td>
                <td>{provider.scopes.length === 0 ? "—" : provider.scopes.join(" ")}</td>
                <td>
                  <label className="visually-hidden" htmlFor={`secret-${provider.name}`}>
                    New client secret for {provider.name}
                  </label>
                  <input
                    id={`secret-${provider.name}`}
                    type="password"
                    autoComplete="off"
                    placeholder={provider.hasSecret ? "Stored — enter to replace" : "Required"}
                    value={provider.newSecret ?? ""}
                    onChange={(event) => {
                      const secret = event.currentTarget.value;
                      edit((current) =>
                        current.map((candidate) =>
                          candidate.name === provider.name
                            ? { ...candidate, newSecret: secret === "" ? null : secret }
                            : candidate,
                        ),
                      );
                    }}
                  />
                  <span className="muted">
                    {provider.newSecret !== null
                      ? " will be replaced on save"
                      : provider.hasSecret
                        ? " stored"
                        : " none yet"}
                  </span>
                </td>
                <td>
                  <input
                    type="checkbox"
                    aria-label={`${provider.name} enabled`}
                    checked={provider.enabled}
                    onChange={(event) => {
                      const enabled = event.currentTarget.checked;
                      edit((current) =>
                        current.map((candidate) =>
                          candidate.name === provider.name ? { ...candidate, enabled } : candidate,
                        ),
                      );
                    }}
                  />
                </td>
                <td>
                  <button
                    type="button"
                    className="link-button"
                    onClick={() =>
                      edit((current) =>
                        current.filter((candidate) => candidate.name !== provider.name),
                      )
                    }
                  >
                    Remove {provider.name}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <p className="muted">
        Register this callback with each provider:{" "}
        <code>
          {`{api}`}/v1/projects/{projectId}/environments/{environmentId}/auth/providers/{"{"}
          name{"}"}/callback
        </code>
        , where <code>{`{api}`}</code> is the environment's API URL shown on Connect.
      </p>

      <form className="stacked-form" onSubmit={addProvider} aria-labelledby="add-provider-heading">
        <h3 id="add-provider-heading">Add a provider</h3>
        <label>
          Name
          <input
            name="name"
            required
            pattern={PROVIDER_NAME_PATTERN}
            placeholder="google"
            autoComplete="off"
          />
        </label>
        <label>
          Kind
          <select name="kind" defaultValue="oidc">
            <option value="oidc">OpenID Connect</option>
            <option value="git_hub">GitHub</option>
          </select>
        </label>
        <label>
          Issuer (OpenID Connect only)
          <input name="issuer" type="url" placeholder="https://accounts.google.com" />
        </label>
        <label>
          Client ID
          <input name="clientId" required autoComplete="off" />
        </label>
        <label>
          Client secret
          <input name="clientSecret" type="password" autoComplete="off" />
        </label>
        <label>
          Scopes (space separated)
          <input name="scopes" placeholder="openid email profile" />
        </label>
        <label>
          <input name="enabled" type="checkbox" defaultChecked /> Enabled
        </label>
        <button type="submit">Add provider</button>
      </form>

      <h2 id="redirects-heading">Redirect URLs</h2>
      <p className="muted">
        One per line. A sign-in may only return users to a page listed here; anything else is
        refused before the provider is contacted. Use <code>https</code>, or <code>http</code> on
        localhost for development.
      </p>
      <label>
        <span className="visually-hidden">Redirect URLs</span>
        <textarea
          aria-labelledby="redirects-heading"
          rows={4}
          value={redirectUrls}
          onChange={(event) => {
            setRedirectUrls(event.currentTarget.value);
            setDirty(true);
            setStatus(null);
          }}
        />
      </label>

      <h2 id="magic-links-heading">Magic links</h2>
      <label>
        <input
          type="checkbox"
          checked={magicLinks.enabled}
          onChange={(event) => {
            const enabled = event.currentTarget.checked;
            setMagicLinks((current) => ({ ...current, enabled }));
            setDirty(true);
            setStatus(null);
          }}
        />{" "}
        Let users sign in by emailed link
      </label>
      <label>
        Link lifetime (seconds)
        <input
          type="number"
          min={60}
          max={3600}
          value={magicLinks.linkTtlSeconds}
          onChange={(event) => {
            const linkTtlSeconds = Number(event.currentTarget.value);
            setMagicLinks((current) => ({ ...current, linkTtlSeconds }));
            setDirty(true);
            setStatus(null);
          }}
        />
      </label>
      <p className="muted">
        The email a magic link goes out in is the environment's magic-link template.
      </p>

      <div className="form-actions">
        <button type="button" onClick={() => void save()} disabled={saving || !dirty}>
          {saving ? "Saving…" : "Save sign-in settings"}
        </button>
        <button
          type="button"
          className="secondary"
          disabled={saving || !dirty || installed === null}
          onClick={() => {
            if (installed !== null) {
              adopt(installed);
              setStatus(null);
            }
          }}
        >
          Discard changes
        </button>
      </div>
    </section>
  );
}

function draftFrom(provider: AuthProviderView): ProviderDraft {
  return {
    name: provider.name,
    kind: provider.kind,
    clientId: provider.clientId,
    scopes: provider.scopes,
    enabled: provider.enabled,
    hasSecret: provider.hasSecret,
    newSecret: null,
  };
}

export function describeKind(kind: AuthProviderKind): string {
  return kind.type === "git_hub" ? "GitHub" : `OpenID Connect · ${kind.issuer}`;
}

function scopesFrom(text: string): string[] {
  return text
    .split(/\s+/u)
    .map((scope) => scope.trim())
    .filter((scope) => scope !== "");
}

function requiredText(data: FormData, name: string): string {
  const value = data.get(name);
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${name} is required`);
  }
  return value.trim();
}

function idempotencyKey(): string {
  return crypto.randomUUID();
}
