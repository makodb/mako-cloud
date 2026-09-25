import type { Collection, WorkspaceSummary } from "@mako-cloud/management-sdk";
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  Eyebrow,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@mako-cloud/ui";
import {
  Activity,
  ArrowRight,
  Database,
  DatabaseBackup,
  Plus,
  RefreshCw,
  SquareFunction,
  Table2,
} from "lucide-react";
import { useEffect, useState } from "react";
import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";

type Section = WorkspaceSummary["sections"][string];
type Resource<T> = { value: T | null; loading: boolean; error: ConsoleApiFailure | null };
const initial = <T,>(): Resource<T> => ({ value: null, loading: true, error: null });

export function DatabaseOverview({
  projectId,
  environmentId,
  navigate,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly navigate: (path: string) => void;
}) {
  const client = useManagementClient();
  const base = `/projects/${projectId}/environments/${environmentId}`;
  const [version, setVersion] = useState(0);
  const [summary, setSummary] = useState(initial<WorkspaceSummary>);
  const [collections, setCollections] = useState(initial<Collection[]>);
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now() / 1000), 10_000);
    return () => window.clearInterval(timer);
  }, []);
  // biome-ignore lint/correctness/useExhaustiveDependencies: version explicitly refreshes both independent providers.
  useEffect(() => {
    let active = true;
    setSummary(initial());
    setCollections(initial());
    void client.getWorkspaceSummary(projectId, environmentId).then(
      (value) => active && setSummary({ value, loading: false, error: null }),
      (error: unknown) =>
        active && setSummary({ value: null, loading: false, error: toConsoleApiFailure(error) }),
    );
    void client.listCollections(projectId, environmentId).then(
      (value) => active && setCollections({ value, loading: false, error: null }),
      (error: unknown) =>
        active &&
        setCollections({ value: null, loading: false, error: toConsoleApiFailure(error) }),
    );
    return () => {
      active = false;
    };
  }, [client, projectId, environmentId, version]);
  const sections = summary.value?.sections ?? {};
  const lifecycle = payload(sections.lifecycle);
  const activity = records(payload(sections.activity).events);
  const samples = records(payload(sections.usage).samples);
  const metrics = [
    {
      id: "lifecycle",
      title: "Environment",
      icon: Database,
      value: lifecycle.ready === true ? "Active" : text(lifecycle.environment, "Not ready"),
      detail: "Project and environment lifecycle",
      path: "settings",
    },
    {
      id: "collections",
      title: "Collections",
      icon: Table2,
      value: count(payload(sections.collections), "count"),
      detail: "Versioned document schemas",
      path: "collections",
    },
    {
      id: "functions",
      title: "Functions",
      icon: SquareFunction,
      value: count(payload(sections.functions), "count"),
      detail: "Functions defined in this environment",
      path: "functions",
    },
    {
      id: "backups",
      title: "Recovery points",
      icon: DatabaseBackup,
      value: count(payload(sections.backups), "verifiedCount"),
      detail: "Verified backups for this environment",
      path: "backups",
    },
  ];
  return (
    <section className="grid min-w-0 gap-6" aria-label="Database overview">
      <header className="flex flex-wrap items-start justify-between gap-4">
        <div className="grid gap-1">
          <Eyebrow>Environment overview</Eyebrow>
          <h1 className="text-2xl">Database overview</h1>
          <p className="m-0 text-sm text-muted-foreground">
            Your collections, database activity, and application services.
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="outline"
            disabled={summary.loading || collections.loading}
            onClick={() => setVersion((value) => value + 1)}
          >
            <RefreshCw aria-hidden="true" />
            Refresh
          </Button>
          <Button onClick={() => navigate(`${base}/connect`)}>
            Connect application
            <ArrowRight aria-hidden="true" />
          </Button>
        </div>
      </header>
      <ApiFailureNotice failure={summary.error} />
      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        {metrics.map((metric) => (
          <Card key={metric.id} as="article" aria-label={metric.title} className="gap-3 py-4">
            <CardHeader className="flex flex-row items-center justify-between px-4">
              <CardTitle as="h2" className="text-sm font-medium text-muted-foreground">
                {metric.title}
              </CardTitle>
              <metric.icon aria-hidden="true" className="size-4 text-muted-foreground" />
            </CardHeader>
            <CardContent className="grid gap-3 px-4">
              <strong className="text-3xl font-semibold tracking-tight">
                {summary.loading
                  ? "…"
                  : readable(sections[metric.id])
                    ? metric.value
                    : "Unavailable"}
              </strong>
              <p className="m-0 text-xs text-muted-foreground">{metric.detail}</p>
              <Observation section={sections[metric.id]} now={now} loading={summary.loading} />
              <a
                href={`${base}/${metric.path}`}
                className="text-xs"
                onClick={(event) => follow(event, `${base}/${metric.path}`, navigate)}
              >
                View {metric.title.toLowerCase()} →
              </a>
            </CardContent>
          </Card>
        ))}
      </div>
      <div className="grid items-start gap-5 xl:grid-cols-[minmax(0,2fr)_minmax(16rem,1fr)]">
        <Card aria-label="Collection inventory" className="min-w-0 overflow-hidden">
          <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-3">
            <div className="grid gap-1">
              <CardTitle>Collections</CardTitle>
              <p className="m-0 text-xs text-muted-foreground">
                Document schemas in this environment
              </p>
            </div>
            <Button size="sm" variant="outline" onClick={() => navigate(`${base}/collections`)}>
              <Plus aria-hidden="true" />
              Create collection
            </Button>
          </CardHeader>
          <CardContent className="grid min-w-0 gap-3">
            <ApiFailureNotice failure={collections.error} />
            {collections.loading ? (
              <p className="text-sm text-muted-foreground" aria-live="polite">
                Loading collections…
              </p>
            ) : collections.value?.length === 0 ? (
              <EmptyState
                title="Create your first collection"
                description="Define a document schema, then add data and connect your application."
                icon={<Database aria-hidden="true" />}
                action={
                  <Button onClick={() => navigate(`${base}/collections`)}>
                    Define a collection
                  </Button>
                }
              />
            ) : collections.value !== null ? (
              <div className="overflow-x-auto rounded-lg border">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Collection</TableHead>
                      <TableHead>Schema</TableHead>
                      <TableHead>State</TableHead>
                      <TableHead>
                        <span className="sr-only">Actions</span>
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {collections.value.map((collection) => (
                      <TableRow key={collection.id}>
                        <TableCell>
                          <a
                            className="flex items-center gap-2 font-mono text-sm"
                            href={`${base}/collections/${collection.id}`}
                            onClick={(event) =>
                              follow(event, `${base}/collections/${collection.id}`, navigate)
                            }
                          >
                            <Table2
                              aria-hidden="true"
                              className="size-4 shrink-0 text-muted-foreground"
                            />
                            {collection.id}
                          </a>
                        </TableCell>
                        <TableCell className="text-xs">v{collection.schemaVersion}</TableCell>
                        <TableCell>
                          <Badge variant={collection.state === "active" ? "positive" : "secondary"}>
                            {collection.state}
                          </Badge>
                        </TableCell>
                        <TableCell>
                          <Button
                            size="sm"
                            variant="ghost"
                            onClick={() => navigate(`${base}/collections/${collection.id}`)}
                          >
                            Schema
                            <ArrowRight aria-hidden="true" />
                          </Button>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
            ) : null}
            {collections.value !== null && collections.value.length > 0 ? (
              <Button className="justify-self-start" onClick={() => navigate(`${base}/data`)}>
                <Database aria-hidden="true" />
                Explore data
              </Button>
            ) : null}
          </CardContent>
        </Card>
        <Card aria-label="Database tools" className="gap-3">
          <CardHeader>
            <CardTitle>Build with your database</CardTitle>
          </CardHeader>
          <CardContent className="grid gap-1">
            {[
              ["data", "Data browser", "Browse documents and run indexed queries"],
              ["collections", "Schema & indexes", "Define fields and prepare query indexes"],
              ["policies", "Access policies", "Control who can read and write documents"],
              ["connect", "Connect an application", "Copy client setup and check your connection"],
            ].map(([path, label, description]) => (
              <a
                key={path}
                href={`${base}/${path}`}
                className="group flex items-center justify-between gap-3 rounded-lg px-3 py-3 text-foreground no-underline hover:bg-muted hover:no-underline"
                onClick={(event) => follow(event, `${base}/${path}`, navigate)}
              >
                <span className="grid gap-1">
                  <strong className="text-sm font-medium">{label}</strong>
                  <span className="text-xs text-muted-foreground">{description}</span>
                </span>
                <ArrowRight
                  aria-hidden="true"
                  className="size-4 shrink-0 text-muted-foreground group-hover:text-primary"
                />
              </a>
            ))}
          </CardContent>
        </Card>
      </div>
      <div className="grid items-start gap-5 xl:grid-cols-2">
        <Card aria-label="Usage observations">
          <CardHeader className="flex flex-row items-center justify-between">
            <CardTitle>Usage observations</CardTitle>
            <a
              href={`${base}/usage`}
              onClick={(event) => follow(event, `${base}/usage`, navigate)}
              className="text-xs"
            >
              Usage & quotas →
            </a>
          </CardHeader>
          <CardContent className="grid gap-4">
            <Observation section={sections.usage} now={now} loading={summary.loading} />
            {readable(sections.usage) ? (
              <>
                {samples.length === 0 ? (
                  <p className="m-0 text-sm text-muted-foreground">
                    No usage measurements available in this window.
                  </p>
                ) : (
                  <dl className="m-0 grid gap-3 sm:grid-cols-2">
                    {samples.map((sample) => (
                      <div
                        key={text(sample.resource, "resource")}
                        className="rounded-lg border bg-muted/20 p-3"
                      >
                        <dt className="text-xs text-muted-foreground">
                          {humanize(text(sample.resource, "Resource"))}
                        </dt>
                        <dd className="m-0 mt-1 text-lg font-semibold">
                          {typeof sample.quantity === "number"
                            ? sample.quantity.toLocaleString()
                            : "Unavailable"}{" "}
                          <span className="text-xs font-normal text-muted-foreground">
                            {text(sample.unit, "")}
                          </span>
                        </dd>
                        <dd className="m-0 mt-1 text-xs text-muted-foreground">
                          Latest reported sample · {timestamp(sample.timestampUnixMilliseconds)}
                        </dd>
                      </div>
                    ))}
                  </dl>
                )}
                <WindowNote value={payload(sections.usage)} />
              </>
            ) : null}
            <div className="grid grid-cols-2 gap-3 border-t pt-4">
              <div>
                <p className="m-0 text-xs text-muted-foreground">Sync error records</p>
                <strong className="text-xl">
                  {readable(sections.sync)
                    ? count(payload(sections.sync), "recordCount")
                    : "Unavailable"}
                </strong>
                <Observation section={sections.sync} now={now} loading={summary.loading} />
                <WindowNote value={payload(sections.sync)} />
                <a
                  href={`${base}/sync`}
                  className="text-xs"
                  onClick={(event) => follow(event, `${base}/sync`, navigate)}
                >
                  Inspect sync →
                </a>
              </div>
              <div>
                <p className="m-0 text-xs text-muted-foreground">Active data jobs</p>
                <strong className="text-xl">
                  {readable(sections.dataJobs)
                    ? count(payload(sections.dataJobs), "active")
                    : "Unavailable"}
                </strong>
                <Observation section={sections.dataJobs} now={now} loading={summary.loading} />
              </div>
            </div>
          </CardContent>
        </Card>
        <Card aria-label="Recent database activity">
          <CardHeader className="flex flex-row items-center justify-between">
            <CardTitle>Recent activity</CardTitle>
            <Activity aria-hidden="true" className="size-4 text-muted-foreground" />
          </CardHeader>
          <CardContent className="grid gap-4">
            <Observation section={sections.activity} now={now} loading={summary.loading} />
            {readable(sections.activity) ? (
              <>
                {activity.length === 0 ? (
                  <p className="m-0 text-sm text-muted-foreground">
                    No activity details available in this window.
                  </p>
                ) : (
                  <ol className="m-0 grid list-none divide-y p-0">
                    {activity.map((event) => (
                      <li
                        key={`${event.timestampUnixMilliseconds}:${event.actorId}:${event.action}:${event.target}`}
                        className="grid gap-1 py-3 first:pt-0"
                      >
                        <div className="flex flex-wrap justify-between gap-2">
                          <strong className="text-sm font-medium">
                            {humanize(text(event.action, "Activity"))}
                          </strong>
                          <span className="text-xs text-muted-foreground">
                            {timestamp(event.timestampUnixMilliseconds)}
                          </span>
                        </div>
                        <span className="truncate font-mono text-xs text-muted-foreground">
                          {text(event.target, "")}
                        </span>
                        <span className="text-xs text-muted-foreground">
                          {text(event.actorId, "")} · {text(event.outcome, "")}
                        </span>
                      </li>
                    ))}
                  </ol>
                )}
                <WindowNote value={payload(sections.activity)} />
              </>
            ) : null}
            <a
              href={`${base}/activity`}
              className="text-xs"
              onClick={(event) => follow(event, `${base}/activity`, navigate)}
            >
              View audit activity →
            </a>
          </CardContent>
        </Card>
      </div>
    </section>
  );
}

function Observation({
  section,
  now,
  loading,
}: {
  readonly section: Section | undefined;
  readonly now: number;
  readonly loading: boolean;
}) {
  const status = loading
    ? "Loading"
    : !readable(section)
      ? "Unavailable"
      : section?.status === "stale" || (section?.freshUntilUnixSeconds ?? 0) <= now
        ? "Stale"
        : "Current";
  return (
    <div className="flex flex-wrap items-center gap-2 text-[11px] text-muted-foreground">
      <Badge
        variant={status === "Current" ? "outline" : status === "Loading" ? "secondary" : "warning"}
        className="text-[10px]"
      >
        {status}
      </Badge>
      {section ? (
        <span>Observed {new Date(section.observedAtUnixSeconds * 1000).toLocaleTimeString()}</span>
      ) : null}
    </div>
  );
}
function readable(section: Section | undefined): boolean {
  return (
    section !== undefined &&
    (section.status === "current" || section.status === "stale") &&
    section.payload != null
  );
}
function payload(section: Section | undefined): Record<string, unknown> {
  return readable(section) ? record(section?.payload) : {};
}
function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}
function records(value: unknown): Record<string, unknown>[] {
  return Array.isArray(value) ? value.map(record) : [];
}
function text(value: unknown, fallback: string): string {
  return typeof value === "string" ? value : fallback;
}
function count(value: Record<string, unknown>, key: string): string {
  return typeof value[key] === "number"
    ? `${value[key].toLocaleString()}${value.limited === true ? "+" : ""}`
    : "Unavailable";
}
function humanize(value: string): string {
  return value.replaceAll("_", " ").replaceAll(".", " ");
}
function timestamp(value: unknown): string {
  return typeof value === "number" ? new Date(value).toLocaleString() : "Time unavailable";
}
function WindowNote({ value }: { readonly value: Record<string, unknown> }) {
  return (
    <p className="m-0 text-xs text-muted-foreground">
      {typeof value.windowStartUnixSeconds === "number" &&
      typeof value.windowEndUnixSeconds === "number"
        ? `${new Date(value.windowStartUnixSeconds * 1000).toLocaleTimeString()} to ${new Date(value.windowEndUnixSeconds * 1000).toLocaleTimeString()}`
        : "Observation window unavailable"}
      {value.limited === true ? " · Partial results, open the detailed view for more" : ""}
    </p>
  );
}
function follow(
  event: React.MouseEvent<HTMLAnchorElement>,
  path: string,
  navigate: (path: string) => void,
) {
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0)
    return;
  event.preventDefault();
  navigate(path);
}
