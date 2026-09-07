import type { ComponentProps } from "react";

import { cn } from "../lib/utils.js";

/** A bordered surface. A `section` by default; an `article` when it is one thing in a list, a `div` when it is neither. */
export function Card({
  className,
  as: Component = "section",
  ...props
}: Omit<ComponentProps<"div">, "ref"> & { as?: "section" | "article" | "div" }) {
  return (
    <Component
      data-slot="card"
      className={cn(
        "flex flex-col gap-5 rounded-xl border bg-card py-5 text-card-foreground shadow-xs",
        className,
      )}
      {...props}
    />
  );
}

export function CardHeader({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="card-header"
      className={cn(
        "grid auto-rows-min items-start gap-1 px-5 has-data-[slot=card-action]:grid-cols-[1fr_auto]",
        className,
      )}
      {...props}
    />
  );
}

/** The card's heading; `as` picks the level the page's outline needs. */
export function CardTitle({
  className,
  as: Component = "h2",
  ...props
}: Omit<ComponentProps<"h2">, "ref"> & { as?: "h1" | "h2" | "h3" | "h4" }) {
  return (
    <Component
      data-slot="card-title"
      className={cn("text-base font-semibold leading-none", className)}
      {...props}
    />
  );
}

export function CardDescription({ className, ...props }: ComponentProps<"p">) {
  return (
    <p
      data-slot="card-description"
      className={cn("text-sm text-muted-foreground", className)}
      {...props}
    />
  );
}

export function CardAction({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="card-action"
      className={cn("col-start-2 row-span-2 row-start-1 self-start justify-self-end", className)}
      {...props}
    />
  );
}

export function CardContent({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="card-content" className={cn("px-5", className)} {...props} />;
}

export function CardFooter({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="card-footer"
      className={cn("flex items-center px-5 [.border-t]:pt-5", className)}
      {...props}
    />
  );
}
