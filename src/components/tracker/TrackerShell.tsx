import { Link, useRouterState } from "@tanstack/react-router";
import type { ReactNode } from "react";

type TrackerShellProps = {
  children?: ReactNode;
  error?: string | null;
};

export function TrackerShell({ children, error = null }: TrackerShellProps) {
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  });

  return (
    <main className="tracker-page">
      <div className="tracker-shell">
        {pathname !== "/auto-detect" ? (
          <div className="flex justify-end">
            <Link
              className="tracker-copy-muted text-[0.625rem] uppercase tracking-[0.3em] transition-colors hover:text-primary"
              to="/auto-detect"
            >
              Auto-detect (experimental)
            </Link>
          </div>
        ) : null}
        {error ? (
          <div className="border border-destructive/60 bg-destructive/10 px-4 py-3 text-sm text-destructive">
            {error}
          </div>
        ) : (
          children
        )}
      </div>
    </main>
  );
}
