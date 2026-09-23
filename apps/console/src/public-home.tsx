import { Button, ThemeToggle, useTheme } from "@mako-cloud/ui";
import {
  ArrowRight,
  BookOpen,
  Boxes,
  Braces,
  Check,
  Cloud,
  Database,
  FileCode2,
  Gauge,
  KeyRound,
  Radio,
  ShieldCheck,
  Sparkles,
  Workflow,
} from "lucide-react";
import type { ComponentType, SVGProps } from "react";

const THEME_KEY = "mako.console.theme";
const USER_BOOK = "/docs/user-book";

type Icon = ComponentType<SVGProps<SVGSVGElement>>;

interface PublicHomeProps {
  readonly navigate: (path: string, replace?: boolean) => void;
}

interface Feature {
  readonly title: string;
  readonly description: string;
  readonly icon: Icon;
  readonly className?: string;
}

interface Guide {
  readonly title: string;
  readonly description: string;
  readonly href: string;
  readonly icon: Icon;
}

const FEATURES: readonly Feature[] = [
  {
    title: "Local-first data",
    description:
      "Sync RxDB clients through pull, push, and a live change stream. Apps keep working when the network drops.",
    icon: Radio,
    className: "lg:col-span-2",
  },
  {
    title: "Project authentication",
    description:
      "Add password, magic-link, GitHub, Google, or OpenID Connect sign-in without mixing app users with your developer team.",
    icon: KeyRound,
  },
  {
    title: "Document policies",
    description:
      "Apply default-deny rules to reads, writes, queries, replication, and conflicts from one policy model.",
    icon: ShieldCheck,
  },
  {
    title: "Server-side building blocks",
    description:
      "Run edge functions, schedules, webhooks, transactional mail, and file storage beside your data.",
    icon: Braces,
    className: "lg:col-span-2",
  },
  {
    title: "Operate from one console",
    description:
      "Review usage, logs, replication health, application users, audit events, and deployment state without changing tools.",
    icon: Gauge,
    className: "lg:col-span-2",
  },
];

const GUIDES: readonly Guide[] = [
  {
    title: "Getting started",
    description:
      "Create a project, define a collection, add a policy, and issue your first public key.",
    href: `${USER_BOOK}#getting-started`,
    icon: Sparkles,
  },
  {
    title: "Connect an RxDB app",
    description: "Set up the supported client and understand pull, push, and live replication.",
    href: `${USER_BOOK}#building-a-local-first-app-with-rxdb`,
    icon: Database,
  },
  {
    title: "Secure application data",
    description: "Learn how application authentication and document policies work together.",
    href: `${USER_BOOK}#document-policies`,
    icon: ShieldCheck,
  },
  {
    title: "Run edge functions",
    description: "Deploy server-side TypeScript with scoped secrets and stable routes.",
    href: `${USER_BOOK}#edge-functions`,
    icon: FileCode2,
  },
];

export function PublicHome({ navigate }: PublicHomeProps) {
  const { resolved, toggle } = useTheme(THEME_KEY);
  const open = (path: string) => (event: React.MouseEvent<HTMLAnchorElement>) => {
    event.preventDefault();
    navigate(path);
  };

  return (
    <div className="min-h-screen overflow-hidden bg-background text-foreground">
      <a
        className="sr-only focus:not-sr-only focus:fixed focus:top-3 focus:left-3 focus:z-50 focus:rounded-md focus:bg-card focus:px-3 focus:py-2 focus:shadow-md"
        href="#main-content"
      >
        Skip to main content
      </a>

      <header className="relative z-20 border-b border-border/70 bg-background/85 backdrop-blur-xl">
        <div className="mx-auto flex h-16 max-w-7xl items-center gap-5 px-5 sm:px-8 lg:px-10">
          <a
            href="/"
            className="flex items-center gap-2.5 text-foreground no-underline"
            aria-label="Mako Cloud home"
          >
            <MakoMark />
            <span className="text-base font-semibold tracking-tight">Mako Cloud</span>
          </a>
          <nav className="ml-auto hidden items-center gap-7 text-sm md:flex" aria-label="Main">
            <a className="text-muted-foreground hover:text-foreground" href="#platform">
              Platform
            </a>
            <a className="text-muted-foreground hover:text-foreground" href="#how-it-works">
              How it works
            </a>
            <a className="text-muted-foreground hover:text-foreground" href="#docs">
              Docs
            </a>
          </nav>
          <div className="ml-auto flex items-center gap-2 md:ml-3">
            <ThemeToggle resolved={resolved} onToggle={toggle} data-testid="public-theme-toggle" />
            <Button variant="ghost" size="sm" asChild>
              <a href="/login" onClick={open("/login")}>
                Sign in
              </a>
            </Button>
            <Button size="sm" asChild className="hidden sm:inline-flex">
              <a href="/create-account" onClick={open("/create-account")}>
                Create account
              </a>
            </Button>
          </div>
        </div>
      </header>

      <main id="main-content" tabIndex={-1} className="outline-none">
        <section className="relative isolate">
          <div className="pointer-events-none absolute inset-0 -z-10 opacity-80" aria-hidden="true">
            <div className="absolute -top-28 left-[8%] size-[30rem] rounded-full bg-primary/12 blur-3xl" />
            <div className="absolute top-40 right-[3%] size-[26rem] rounded-full bg-chart-2/10 blur-3xl" />
            <div className="absolute inset-x-0 top-0 h-[40rem] bg-[linear-gradient(to_right,var(--border)_1px,transparent_1px),linear-gradient(to_bottom,var(--border)_1px,transparent_1px)] bg-[size:64px_64px] mask-[linear-gradient(to_bottom,black,transparent_88%)] opacity-30" />
          </div>

          <div className="mx-auto grid max-w-7xl items-center gap-14 px-5 py-20 sm:px-8 sm:py-28 lg:grid-cols-[1.02fr_0.98fr] lg:px-10 lg:py-32">
            <div className="max-w-3xl">
              <div className="mb-7 inline-flex items-center gap-2 rounded-full border bg-card/80 px-3 py-1.5 text-xs font-medium text-muted-foreground shadow-xs">
                <span className="size-1.5 rounded-full bg-positive" aria-hidden="true" />
                Public preview
              </div>
              <h1 className="max-w-3xl text-4xl leading-[1.06] font-semibold tracking-[-0.045em] text-balance sm:text-6xl lg:text-7xl">
                The cloud backend for apps that work offline.
              </h1>
              <p className="mt-7 max-w-2xl text-lg leading-8 text-muted-foreground sm:text-xl">
                Mako Cloud gives your RxDB app sync, user authentication, document policies, file
                storage, and server-side functions in one project.
              </p>
              <div className="mt-9 flex flex-col gap-3 sm:flex-row">
                <Button size="lg" asChild className="group">
                  <a href="/create-account" onClick={open("/create-account")}>
                    Start building
                    <ArrowRight className="transition-transform group-hover:translate-x-0.5" />
                  </a>
                </Button>
                <Button size="lg" variant="outline" asChild>
                  <a href={`${USER_BOOK}#getting-started`}>
                    <BookOpen />
                    Read the quickstart
                  </a>
                </Button>
              </div>
              <ul className="mt-8 flex flex-wrap gap-x-6 gap-y-2 p-0 text-sm text-muted-foreground">
                {["Typed TypeScript clients", "Default-deny data access", "Built for RxDB"].map(
                  (item) => (
                    <li key={item} className="flex list-none items-center gap-2">
                      <Check className="size-4 text-positive" aria-hidden="true" />
                      {item}
                    </li>
                  ),
                )}
              </ul>
            </div>

            <PlatformPreview />
          </div>
        </section>

        <section id="platform" className="border-y bg-card/45 scroll-mt-16">
          <div className="mx-auto max-w-7xl px-5 py-20 sm:px-8 sm:py-24 lg:px-10">
            <div className="max-w-2xl">
              <p className="m-0 text-xs font-semibold tracking-[0.18em] text-primary uppercase">
                One project, one control plane
              </p>
              <h2 className="mt-3 text-3xl tracking-[-0.03em] sm:text-4xl">
                The parts your application needs already agree with each other.
              </h2>
              <p className="mt-5 text-base leading-7 text-muted-foreground">
                Define your data once, enforce access on every path, and use the same environment
                through the console, CLI, API, and client SDK.
              </p>
            </div>

            <div className="mt-12 grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
              {FEATURES.map((feature) => (
                <FeatureCard key={feature.title} feature={feature} />
              ))}
            </div>
          </div>
        </section>

        <section id="how-it-works" className="scroll-mt-16">
          <div className="mx-auto grid max-w-7xl gap-12 px-5 py-20 sm:px-8 sm:py-24 lg:grid-cols-[0.85fr_1.15fr] lg:items-center lg:px-10">
            <div className="max-w-xl">
              <p className="m-0 text-xs font-semibold tracking-[0.18em] text-primary uppercase">
                A shorter path to a working app
              </p>
              <h2 className="mt-3 text-3xl tracking-[-0.03em] sm:text-4xl">
                Build locally. Sync when connected. Keep control on the server.
              </h2>
              <p className="mt-5 text-base leading-7 text-muted-foreground">
                Your UI reads and writes its local RxDB database. Mako moves approved changes,
                resolves checkpoints, and streams new data back to the device.
              </p>
              <Button variant="outline" asChild className="mt-7">
                <a href={`${USER_BOOK}#building-a-local-first-app-with-rxdb`}>
                  See the RxDB guide
                  <ArrowRight />
                </a>
              </Button>
            </div>

            <ol className="grid gap-3 p-0 sm:grid-cols-3" aria-label="How Mako Cloud works">
              <FlowStep
                number="01"
                title="Model"
                description="Create a JSON Schema, indexes, and access policy for each collection."
                icon={Boxes}
              />
              <FlowStep
                number="02"
                title="Connect"
                description="Add the Mako RxDB adapter with your environment URL and public key."
                icon={Workflow}
              />
              <FlowStep
                number="03"
                title="Operate"
                description="Inspect usage, logs, users, functions, and data from the console."
                icon={Cloud}
              />
            </ol>
          </div>
        </section>

        <section id="docs" className="border-t bg-secondary/45 scroll-mt-16">
          <div className="mx-auto max-w-7xl px-5 py-20 sm:px-8 sm:py-24 lg:px-10">
            <div className="flex flex-col gap-5 sm:flex-row sm:items-end sm:justify-between">
              <div className="max-w-2xl">
                <p className="m-0 text-xs font-semibold tracking-[0.18em] text-primary uppercase">
                  Cloud user docs
                </p>
                <h2 className="mt-3 text-3xl tracking-[-0.03em] sm:text-4xl">
                  Start with the task in front of you.
                </h2>
                <p className="mt-5 text-base leading-7 text-muted-foreground">
                  The User Book covers the console, CLI, API, SDKs, limits, and every hosted
                  capability.
                </p>
              </div>
              <Button variant="outline" asChild>
                <a href={USER_BOOK}>
                  Open the full User Book
                  <ArrowRight />
                </a>
              </Button>
            </div>

            <div className="mt-10 grid gap-px overflow-hidden rounded-xl border bg-border sm:grid-cols-2 lg:grid-cols-4">
              {GUIDES.map((guide) => (
                <GuideLink key={guide.title} guide={guide} />
              ))}
            </div>
          </div>
        </section>

        <section className="border-t">
          <div className="mx-auto flex max-w-7xl flex-col gap-7 px-5 py-16 sm:px-8 md:flex-row md:items-center md:justify-between lg:px-10">
            <div>
              <h2 className="text-2xl tracking-[-0.025em]">Ready to connect your first project?</h2>
              <p className="mt-2 text-sm text-muted-foreground">
                Create a developer account to enter the hosted cloud preview.
              </p>
            </div>
            <div className="flex flex-col gap-3 sm:flex-row">
              <Button variant="outline" asChild>
                <a href="/login" onClick={open("/login")}>
                  Sign in
                </a>
              </Button>
              <Button asChild>
                <a href="/create-account" onClick={open("/create-account")}>
                  Create developer account
                  <ArrowRight />
                </a>
              </Button>
            </div>
          </div>
        </section>
      </main>

      <footer className="border-t bg-card">
        <div className="mx-auto flex max-w-7xl flex-col gap-4 px-5 py-8 text-sm text-muted-foreground sm:flex-row sm:items-center sm:justify-between sm:px-8 lg:px-10">
          <div className="flex items-center gap-2.5 text-foreground">
            <MakoMark small />
            <span className="font-medium">Mako Cloud</span>
          </div>
          <p className="m-0">Cloud test environment · Public preview</p>
          <nav className="flex flex-wrap gap-5" aria-label="Footer">
            <a className="text-muted-foreground hover:text-foreground" href="#docs">
              User docs
            </a>
            <a
              className="text-muted-foreground hover:text-foreground"
              href="/login"
              onClick={open("/login")}
            >
              Sign in
            </a>
          </nav>
        </div>
      </footer>
    </div>
  );
}

function MakoMark({ small = false }: { readonly small?: boolean }) {
  return (
    <span
      className={`${small ? "size-7 rounded-lg" : "size-8 rounded-[0.6rem]"} grid shrink-0 place-items-center bg-primary text-primary-foreground shadow-sm shadow-primary/20`}
      aria-hidden="true"
    >
      <span
        className={`${small ? "size-2.5" : "size-3"} rotate-45 rounded-[0.2rem] border-2 border-current border-t-transparent`}
      />
    </span>
  );
}

function PlatformPreview() {
  return (
    <div
      className="relative mx-auto w-full max-w-xl"
      role="img"
      aria-label="Mako Cloud platform overview"
    >
      <div className="absolute inset-10 -z-10 rounded-full bg-primary/20 blur-3xl" />
      <div className="overflow-hidden rounded-2xl border bg-card/90 shadow-2xl shadow-primary/10 backdrop-blur">
        <div className="flex h-11 items-center gap-2 border-b px-4">
          <span className="size-2 rounded-full bg-destructive/70" />
          <span className="size-2 rounded-full bg-warning/70" />
          <span className="size-2 rounded-full bg-positive/70" />
          <span className="ml-3 font-mono text-[11px] text-muted-foreground">
            production / overview
          </span>
        </div>
        <div className="grid gap-4 p-4 sm:p-6">
          <div className="flex items-start justify-between gap-4">
            <div>
              <p className="m-0 text-[11px] font-semibold tracking-wider text-muted-foreground uppercase">
                Environment
              </p>
              <p className="mt-1 text-base font-semibold">Production</p>
            </div>
            <span className="inline-flex items-center gap-1.5 rounded-full bg-positive/10 px-2.5 py-1 text-xs font-medium text-positive">
              <span className="size-1.5 rounded-full bg-positive" />
              Active
            </span>
          </div>

          <div className="grid grid-cols-3 gap-2">
            <PreviewMetric label="Collections" value="8" />
            <PreviewMetric label="Sync clients" value="1,284" />
            <PreviewMetric label="Functions" value="5" />
          </div>

          <div className="rounded-xl border bg-background/70 p-4">
            <div className="mb-5 flex items-center justify-between">
              <div>
                <p className="m-0 text-xs font-medium">Live replication</p>
                <p className="mt-1 text-[11px] text-muted-foreground">Documents synced today</p>
              </div>
              <Radio className="size-4 text-positive" />
            </div>
            <div className="flex h-24 items-end gap-1.5" aria-hidden="true">
              {[35, 48, 42, 62, 54, 78, 70, 86, 68, 92, 76, 88, 81, 96, 90, 100].map(
                (height, index) => (
                  <span
                    key={height}
                    className="min-w-0 flex-1 rounded-t-sm bg-primary/75"
                    style={{ height: `${height}%`, opacity: 0.42 + index * 0.035 }}
                  />
                ),
              )}
            </div>
          </div>

          <div className="grid gap-2 sm:grid-cols-2">
            <PreviewStatus icon={Database} title="Data & sync" detail="All systems normal" />
            <PreviewStatus icon={ShieldCheck} title="Policies" detail="8 collections protected" />
          </div>
        </div>
      </div>

      <div className="absolute -right-3 -bottom-5 hidden items-center gap-3 rounded-xl border bg-card px-4 py-3 shadow-lg sm:flex">
        <span className="grid size-8 place-items-center rounded-lg bg-positive/10 text-positive">
          <Check className="size-4" />
        </span>
        <span>
          <span className="block text-xs font-semibold">Client connected</span>
          <span className="block text-[11px] text-muted-foreground">Live sync is ready</span>
        </span>
      </div>
    </div>
  );
}

function PreviewMetric({ label, value }: { readonly label: string; readonly value: string }) {
  return (
    <div className="rounded-lg border bg-background/70 px-3 py-3">
      <p className="m-0 truncate text-[10px] text-muted-foreground sm:text-[11px]">{label}</p>
      <p className="mt-1 truncate text-base font-semibold tabular-nums sm:text-lg">{value}</p>
    </div>
  );
}

function PreviewStatus({
  icon: Icon,
  title,
  detail,
}: {
  readonly icon: Icon;
  readonly title: string;
  readonly detail: string;
}) {
  return (
    <div className="flex items-center gap-3 rounded-lg border bg-background/70 p-3">
      <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-primary/10 text-primary">
        <Icon className="size-4" />
      </span>
      <span className="min-w-0">
        <span className="block truncate text-xs font-medium">{title}</span>
        <span className="block truncate text-[11px] text-muted-foreground">{detail}</span>
      </span>
    </div>
  );
}

function FeatureCard({ feature }: { readonly feature: Feature }) {
  const Icon = feature.icon;
  return (
    <article
      className={`group min-h-56 rounded-xl border bg-card p-6 shadow-xs transition-[transform,box-shadow,border-color] hover:-translate-y-0.5 hover:border-primary/30 hover:shadow-md ${feature.className ?? ""}`}
    >
      <span className="grid size-10 place-items-center rounded-lg bg-primary/10 text-primary transition-colors group-hover:bg-primary group-hover:text-primary-foreground">
        <Icon className="size-5" aria-hidden="true" />
      </span>
      <h3 className="mt-9 text-lg">{feature.title}</h3>
      <p className="mt-3 max-w-xl text-sm leading-6 text-muted-foreground">{feature.description}</p>
    </article>
  );
}

function FlowStep({
  number,
  title,
  description,
  icon: Icon,
}: {
  readonly number: string;
  readonly title: string;
  readonly description: string;
  readonly icon: Icon;
}) {
  return (
    <li className="relative flex min-h-64 list-none flex-col rounded-xl border bg-card p-5 shadow-xs">
      <div className="flex items-start justify-between">
        <span className="font-mono text-xs text-muted-foreground">{number}</span>
        <span className="grid size-9 place-items-center rounded-lg bg-primary/10 text-primary">
          <Icon className="size-4" aria-hidden="true" />
        </span>
      </div>
      <div className="mt-auto pt-12">
        <h3 className="text-base">{title}</h3>
        <p className="mt-2 text-sm leading-6 text-muted-foreground">{description}</p>
      </div>
    </li>
  );
}

function GuideLink({ guide }: { readonly guide: Guide }) {
  const Icon = guide.icon;
  return (
    <a
      className="group flex min-h-56 flex-col bg-card p-5 text-foreground no-underline transition-colors hover:bg-accent"
      href={guide.href}
    >
      <span className="flex items-start justify-between">
        <span className="grid size-9 place-items-center rounded-lg bg-primary/10 text-primary">
          <Icon className="size-4" aria-hidden="true" />
        </span>
        <ArrowRight className="size-4 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
      </span>
      <span className="mt-auto pt-10">
        <span className="block text-base font-semibold">{guide.title}</span>
        <span className="mt-2 block text-sm leading-6 text-muted-foreground">
          {guide.description}
        </span>
      </span>
    </a>
  );
}
