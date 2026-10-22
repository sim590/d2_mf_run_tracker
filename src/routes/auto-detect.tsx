import { createFileRoute, Link } from "@tanstack/react-router";

import { AutoDetectPanel } from "@/components/auto-detect/AutoDetectPanel";
import { TrackerShell } from "@/components/tracker/TrackerShell";

export const Route = createFileRoute("/auto-detect")({
  component: AutoDetectRoute,
});

function AutoDetectRoute() {
  return (
    <TrackerShell>
      <AutoDetectPanel />
      <div className="text-center">
        <Link
          className="tracker-copy-muted text-xs uppercase tracking-[0.2em] transition-colors hover:text-primary"
          to="/"
        >
          Back to sessions
        </Link>
      </div>
    </TrackerShell>
  );
}
