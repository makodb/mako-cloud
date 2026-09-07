// The kit's public surface: every component the design system promises is
// exported and callable, and the helpers behave.
import assert from "node:assert/strict";
import { test } from "node:test";

import * as kit from "../dist/index.js";

const COMPONENTS = [
  "Button",
  "Input",
  "Textarea",
  "Label",
  "Field",
  "Badge",
  "Card",
  "CardHeader",
  "CardTitle",
  "CardDescription",
  "CardAction",
  "CardContent",
  "CardFooter",
  "Eyebrow",
  "Separator",
  "Skeleton",
  "Dialog",
  "DialogTrigger",
  "DialogContent",
  "DialogHeader",
  "DialogFooter",
  "DialogTitle",
  "DialogDescription",
  "DialogClose",
  "Sheet",
  "SheetTrigger",
  "SheetContent",
  "SheetHeader",
  "SheetBody",
  "SheetFooter",
  "SheetTitle",
  "SheetDescription",
  "SheetClose",
  "DropdownMenu",
  "DropdownMenuTrigger",
  "DropdownMenuContent",
  "DropdownMenuItem",
  "DropdownMenuCheckboxItem",
  "DropdownMenuRadioGroup",
  "DropdownMenuRadioItem",
  "DropdownMenuLabel",
  "DropdownMenuSeparator",
  "Popover",
  "PopoverTrigger",
  "PopoverContent",
  "Tooltip",
  "TooltipTrigger",
  "TooltipContent",
  "Tabs",
  "TabsList",
  "TabsLine",
  "TabsTrigger",
  "TabsContent",
  "NativeSelect",
  "Select",
  "SelectTrigger",
  "SelectValue",
  "SelectContent",
  "SelectItem",
  "Checkbox",
  "Switch",
  "Progress",
  "Table",
  "TableHeader",
  "TableBody",
  "TableFooter",
  "TableRow",
  "TableHead",
  "TableCell",
  "TableCaption",
  "Alert",
  "AlertTitle",
  "AlertDescription",
  "EmptyState",
  "Avatar",
  "AvatarImage",
  "AvatarFallback",
  "ToastProvider",
  "ThemeToggle",
  "LineChart",
  "AreaChart",
  "BarChart",
  "DonutChart",
  "Sparkline",
];

test("every component the kit promises is exported as something React can render", () => {
  for (const name of COMPONENTS) {
    const exported = kit[name];
    assert.ok(exported !== undefined, `${name} is not exported`);
    const kind = typeof exported;
    assert.ok(
      kind === "function" || (kind === "object" && exported !== null),
      `${name} is a ${kind}, not a component`,
    );
  }
  for (const hook of ["useTheme", "useToast", "cn", "initials", "seriesColor", "motionAllowed"]) {
    assert.equal(typeof kit[hook], "function", `${hook} is not exported as a function`);
  }
});

test("cn lets a later utility win over an earlier one of the same kind", () => {
  assert.equal(kit.cn("p-2", "p-4"), "p-4");
  assert.equal(kit.cn("text-sm", false, undefined, "font-medium"), "text-sm font-medium");
  assert.equal(kit.cn("bg-primary", { "bg-muted": true }), "bg-muted");
});

test("initials reduce a name to at most two letters", () => {
  assert.equal(kit.initials("Whole Foods"), "WF");
  assert.equal(kit.initials("Netflix"), "N");
  assert.equal(kit.initials("  amazon web services "), "AS");
  assert.equal(kit.initials(""), "?");
});

test("motion is never allowed outside a browser", () => {
  assert.equal(kit.motionAllowed(), false);
});
