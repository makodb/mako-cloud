import { XIcon } from "lucide-react";
import { Toast as ToastPrimitive } from "radix-ui";
import { type ReactNode, createContext, useCallback, useContext, useMemo, useState } from "react";

import { cn } from "../lib/utils.js";

export interface ToastOptions {
  title: string;
  description?: string;
  variant?: "default" | "destructive" | "positive";
  /** Milliseconds before it leaves on its own; errors stay until dismissed. */
  duration?: number;
}

interface ToastRecord extends ToastOptions {
  id: number;
}

const ToastContext = createContext<((options: ToastOptions) => void) | null>(null);

/**
 * Brief confirmations in a corner: "Rule applied to 12 transactions". Mount
 * one provider at the root; `useToast()` anywhere under it returns the
 * function that shows one.
 */
export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastRecord[]>([]);
  const toast = useCallback((options: ToastOptions) => {
    setToasts((current) => [...current, { ...options, id: Date.now() + Math.random() }]);
  }, []);
  const dismiss = useCallback((id: number) => {
    setToasts((current) => current.filter((entry) => entry.id !== id));
  }, []);
  const value = useMemo(() => toast, [toast]);
  return (
    <ToastContext.Provider value={value}>
      <ToastPrimitive.Provider swipeDirection="right">
        {children}
        {toasts.map((entry) => (
          <ToastPrimitive.Root
            key={entry.id}
            duration={entry.duration ?? (entry.variant === "destructive" ? Infinity : 5000)}
            onOpenChange={(open) => {
              if (!open) dismiss(entry.id);
            }}
            className={cn(
              "group pointer-events-auto relative flex w-full items-start gap-3 overflow-hidden rounded-lg border p-4 pr-8 shadow-lg transition-all data-[swipe=cancel]:translate-x-0 data-[swipe=end]:translate-x-(--radix-toast-swipe-end-x) data-[swipe=move]:translate-x-(--radix-toast-swipe-move-x) data-[swipe=move]:transition-none data-[state=closed]:animate-out data-[state=closed]:fade-out-80 data-[state=closed]:slide-out-to-right-full data-[state=open]:animate-in data-[state=open]:slide-in-from-bottom-full",
              entry.variant === "destructive"
                ? "border-destructive/40 bg-destructive text-destructive-foreground"
                : entry.variant === "positive"
                  ? "border-positive/40 bg-card text-card-foreground"
                  : "bg-card text-card-foreground",
            )}
          >
            <div className="grid gap-1">
              <ToastPrimitive.Title className="text-sm font-semibold">
                {entry.title}
              </ToastPrimitive.Title>
              {entry.description ? (
                <ToastPrimitive.Description className="text-sm opacity-90">
                  {entry.description}
                </ToastPrimitive.Description>
              ) : null}
            </div>
            <ToastPrimitive.Close
              aria-label="Dismiss"
              className="absolute top-2 right-2 rounded-md p-1 opacity-70 transition-opacity hover:opacity-100 focus:opacity-100 focus:outline-hidden focus:ring-2 focus:ring-ring"
            >
              <XIcon className="size-4" />
            </ToastPrimitive.Close>
          </ToastPrimitive.Root>
        ))}
        <ToastPrimitive.Viewport className="fixed right-0 bottom-0 z-[100] flex max-h-screen w-full flex-col gap-2 p-4 sm:max-w-sm" />
      </ToastPrimitive.Provider>
    </ToastContext.Provider>
  );
}

export function useToast(): (options: ToastOptions) => void {
  const toast = useContext(ToastContext);
  if (toast === null) {
    throw new Error("useToast needs a ToastProvider above it");
  }
  return toast;
}
