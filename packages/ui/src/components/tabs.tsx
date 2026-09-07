import { Tabs as TabsPrimitive } from "radix-ui";
import type { ComponentProps } from "react";

import { cn } from "../lib/utils.js";

export function Tabs({ className, ...props }: ComponentProps<typeof TabsPrimitive.Root>) {
  return (
    <TabsPrimitive.Root
      data-slot="tabs"
      className={cn("flex flex-col gap-2", className)}
      {...props}
    />
  );
}

export function TabsList({ className, ...props }: ComponentProps<typeof TabsPrimitive.List>) {
  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      className={cn(
        "inline-flex h-9 w-fit items-center justify-center rounded-lg bg-muted p-[3px] text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}

export function TabsTrigger({ className, ...props }: ComponentProps<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      data-slot="tabs-trigger"
      className={cn(
        "inline-flex h-[calc(100%-1px)] flex-1 items-center justify-center gap-1.5 whitespace-nowrap rounded-md border border-transparent px-2 py-1 text-sm font-medium text-foreground/70 transition-[color,box-shadow] focus-visible:border-ring focus-visible:outline-1 focus-visible:outline-ring focus-visible:ring-[3px] focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 data-[state=active]:bg-card data-[state=active]:text-foreground data-[state=active]:shadow-sm [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
        className,
      )}
      {...props}
    />
  );
}

export function TabsContent({ className, ...props }: ComponentProps<typeof TabsPrimitive.Content>) {
  return (
    <TabsPrimitive.Content
      data-slot="tabs-content"
      className={cn("flex-1 outline-none", className)}
      {...props}
    />
  );
}

/**
 * Tabs drawn as a row of underlined links rather than a pill group: for a
 * screen's own sections (a settings hub, a transactions view) where the pill
 * would fight the page header.
 */
export function TabsLine({ className, ...props }: ComponentProps<typeof TabsPrimitive.List>) {
  return (
    <TabsPrimitive.List
      data-slot="tabs-line"
      className={cn(
        "flex w-full items-end gap-4 border-b text-sm [&>[data-slot=tabs-trigger]]:h-9 [&>[data-slot=tabs-trigger]]:flex-none [&>[data-slot=tabs-trigger]]:rounded-none [&>[data-slot=tabs-trigger]]:border-0 [&>[data-slot=tabs-trigger]]:border-b-2 [&>[data-slot=tabs-trigger]]:border-transparent [&>[data-slot=tabs-trigger]]:px-1 [&>[data-slot=tabs-trigger]]:pb-2 [&>[data-slot=tabs-trigger]]:text-muted-foreground [&>[data-slot=tabs-trigger]]:shadow-none [&>[data-slot=tabs-trigger][data-state=active]]:border-primary [&>[data-slot=tabs-trigger][data-state=active]]:bg-transparent [&>[data-slot=tabs-trigger][data-state=active]]:text-foreground",
        className,
      )}
      {...props}
    />
  );
}
