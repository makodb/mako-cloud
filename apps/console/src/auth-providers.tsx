import type {
  AuthProviderKind,
  AuthProviderView,
  AuthSettings,
  AuthSettingsUpdate,
} from "@mako-cloud/management-sdk";
import {
  Alert,
  AlertDescription,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Checkbox,
  Eyebrow,
  Field,
  Input,
  Label,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Textarea,
} from "@mako-cloud/ui";
import { CircleCheck, Plus } from "lucide-react";
import { type FormEvent, useCallback, useEffect, useState } from "react";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";

const PROVIDER_NAME_PATTERN = "[a-z][a-z0-9\\-]{1,63}";
const DEFAULT_LINK_TTL_SECONDS = 900;

/** An identifier the developer will copy: a name, a client id, a URL. */
const MONO = "rounded-sm bg-muted px-1 py-0.5 font-mono text-[0.85em]";

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
  const [verifyEmail, setVerifyEmail] = useState(false);
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
    setVerifyEmail(settings.emailVerification.required);
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
        emailVerification: { required: verifyEmail },
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
    <section aria-labelledby="auth-providers-title" className="grid gap-6">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="grid max-w-3xl gap-1">
          <Eyebrow>Environment {environmentId}</Eyebrow>
          <h1 id="auth-providers-title" className="text-2xl">
            Auth providers
          </h1>
          <p className="m-0 text-sm text-muted-foreground">
            How your application's users sign in besides email and password: external OpenID Connect
            and GitHub providers, the pages the sign-in flow may return to, and magic links. Client
            secrets are sealed on save and never shown again.
          </p>
        </div>
        {installed === null ? null : (
          <p className="m-0 text-sm text-muted-foreground tabular-nums">
            Installed version {installed.version}
          </p>
        )}
      </div>
      {failure === null ? null : <ApiFailureNotice failure={failure} />}
      {status === null ? null : (
        <Alert variant="positive" role="status">
          <CircleCheck aria-hidden="true" />
          <AlertDescription>{status}</AlertDescription>
        </Alert>
      )}
      {installed === null && failure === null ? (
        <p role="status" className="m-0 text-sm text-muted-foreground">
          Loading sign-in settings…
        </p>
      ) : null}

      <Card>
        <CardHeader>
          <CardTitle id="providers-heading">Sign-in providers</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-4">
          {providers.length === 0 ? (
            <p className="m-0 text-sm text-muted-foreground">
              No external providers yet. Add one below.
            </p>
          ) : (
            <Table aria-labelledby="providers-heading">
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead scope="col">Provider</TableHead>
                  <TableHead scope="col">Kind</TableHead>
                  <TableHead scope="col">Client ID</TableHead>
                  <TableHead scope="col">Scopes</TableHead>
                  <TableHead scope="col">Secret</TableHead>
                  <TableHead scope="col">Enabled</TableHead>
                  <TableHead scope="col">
                    <span className="sr-only">Actions</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {providers.map((provider) => (
                  <TableRow key={provider.name}>
                    <TableHead scope="row">
                      <code className="font-mono text-sm">{provider.name}</code>
                    </TableHead>
                    <TableCell className="whitespace-normal">
                      {describeKind(provider.kind)}
                    </TableCell>
                    <TableCell>
                      <code className="font-mono text-xs">{provider.clientId}</code>
                    </TableCell>
                    <TableCell className="whitespace-normal font-mono text-xs text-muted-foreground">
                      {provider.scopes.length === 0 ? "—" : provider.scopes.join(" ")}
                    </TableCell>
                    <TableCell className="whitespace-normal">
                      <Label className="sr-only" htmlFor={`secret-${provider.name}`}>
                        New client secret for {provider.name}
                      </Label>
                      <Input
                        id={`secret-${provider.name}`}
                        type="password"
                        autoComplete="off"
                        className="w-60"
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
                      <span className="block text-xs text-muted-foreground">
                        {provider.newSecret !== null
                          ? " will be replaced on save"
                          : provider.hasSecret
                            ? " stored"
                            : " none yet"}
                      </span>
                    </TableCell>
                    <TableCell>
                      <Checkbox
                        aria-label={`${provider.name} enabled`}
                        checked={provider.enabled}
                        onCheckedChange={(checked) => {
                          const enabled = checked === true;
                          edit((current) =>
                            current.map((candidate) =>
                              candidate.name === provider.name
                                ? { ...candidate, enabled }
                                : candidate,
                            ),
                          );
                        }}
                      />
                    </TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-muted-foreground hover:text-destructive"
                        onClick={() =>
                          edit((current) =>
                            current.filter((candidate) => candidate.name !== provider.name),
                          )
                        }
                      >
                        Remove {provider.name}
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
          <p className="m-0 text-sm text-muted-foreground">
            Register this callback with each provider:{" "}
            <code className={`${MONO} break-all`}>
              {`{api}`}/v1/projects/{projectId}/environments/{environmentId}/auth/providers/{"{"}
              name{"}"}/callback
            </code>
            , where <code className={MONO}>{`{api}`}</code> is the environment's API URL shown on
            Connect.
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle as="h3" id="add-provider-heading">
            Add a provider
          </CardTitle>
        </CardHeader>
        <CardContent>
          <form
            className="grid gap-4"
            onSubmit={addProvider}
            aria-labelledby="add-provider-heading"
          >
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="Name" htmlFor="add-provider-name">
                <Input
                  id="add-provider-name"
                  name="name"
                  required
                  pattern={PROVIDER_NAME_PATTERN}
                  placeholder="google"
                  autoComplete="off"
                  className="font-mono"
                />
              </Field>
              <Field label="Kind" htmlFor="add-provider-kind">
                <NativeSelect id="add-provider-kind" name="kind" defaultValue="oidc">
                  <option value="oidc">OpenID Connect</option>
                  <option value="git_hub">GitHub</option>
                </NativeSelect>
              </Field>
              <Field label="Issuer (OpenID Connect only)" htmlFor="add-provider-issuer">
                <Input
                  id="add-provider-issuer"
                  name="issuer"
                  type="url"
                  placeholder="https://accounts.google.com"
                  className="font-mono"
                />
              </Field>
              <Field label="Client ID" htmlFor="add-provider-client-id">
                <Input
                  id="add-provider-client-id"
                  name="clientId"
                  required
                  autoComplete="off"
                  className="font-mono"
                />
              </Field>
              <Field label="Client secret" htmlFor="add-provider-client-secret">
                <Input
                  id="add-provider-client-secret"
                  name="clientSecret"
                  type="password"
                  autoComplete="off"
                />
              </Field>
              <Field label="Scopes (space separated)" htmlFor="add-provider-scopes">
                <Input
                  id="add-provider-scopes"
                  name="scopes"
                  placeholder="openid email profile"
                  className="font-mono"
                />
              </Field>
            </div>
            <div className="flex items-center gap-2">
              <Checkbox id="add-provider-enabled" name="enabled" defaultChecked />
              <Label htmlFor="add-provider-enabled">Enabled</Label>
            </div>
            <Button type="submit" variant="secondary" className="justify-self-start">
              <Plus aria-hidden="true" />
              Add provider
            </Button>
          </form>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle id="redirects-heading">Redirect URLs</CardTitle>
          <CardDescription>
            One per line. A sign-in may only return users to a page listed here; anything else is
            refused before the provider is contacted. Use <code className={MONO}>https</code>, or{" "}
            <code className={MONO}>http</code> on localhost for development.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <Label className="sr-only" htmlFor="redirect-urls">
            Redirect URLs
          </Label>
          <Textarea
            id="redirect-urls"
            aria-labelledby="redirects-heading"
            rows={4}
            className="min-h-24 font-mono text-xs leading-relaxed"
            value={redirectUrls}
            onChange={(event) => {
              setRedirectUrls(event.currentTarget.value);
              setDirty(true);
              setStatus(null);
            }}
          />
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle id="magic-links-heading">Magic links</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-4">
          <div className="flex items-center gap-2">
            <Checkbox
              id="magic-links-enabled"
              checked={magicLinks.enabled}
              onCheckedChange={(checked) => {
                const enabled = checked === true;
                setMagicLinks((current) => ({ ...current, enabled }));
                setDirty(true);
                setStatus(null);
              }}
            />
            <Label htmlFor="magic-links-enabled">Let users sign in by emailed link</Label>
          </div>
          <Field label="Link lifetime (seconds)" htmlFor="magic-link-ttl" className="max-w-xs">
            <Input
              id="magic-link-ttl"
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
          </Field>
          <p className="m-0 text-sm text-muted-foreground">
            The email a magic link goes out in is the environment's magic-link template.
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle id="email-verification-heading">Email verification</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-4">
          <div className="flex items-center gap-2">
            <Checkbox
              id="email-verification-required"
              checked={verifyEmail}
              onCheckedChange={(checked) => {
                setVerifyEmail(checked === true);
                setDirty(true);
                setStatus(null);
              }}
            />
            <Label htmlFor="email-verification-required">
              Require new accounts to confirm their email before signing in
            </Label>
          </div>
          <p className="m-0 text-sm text-muted-foreground">
            Sign-up then names one of the redirect URLs above and the link lands there with{" "}
            <code>#verification_token=…</code>; the app redeems it with{" "}
            <code>POST …/auth/verify-email</code>. The email is the environment's verification
            template.
          </p>
        </CardContent>
      </Card>

      <div className="flex flex-wrap gap-2">
        <Button onClick={() => void save()} disabled={saving || !dirty}>
          {saving ? "Saving…" : "Save sign-in settings"}
        </Button>
        <Button
          variant="outline"
          disabled={saving || !dirty || installed === null}
          onClick={() => {
            if (installed !== null) {
              adopt(installed);
              setStatus(null);
            }
          }}
        >
          Discard changes
        </Button>
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
