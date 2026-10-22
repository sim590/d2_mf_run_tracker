import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useEffect, useState } from "react";

import { ActiveSessionScreen } from "@/components/tracker/ActiveSessionScreen";
import { TrackerShell } from "@/components/tracker/TrackerShell";
import { useTrackerData } from "@/hooks/useTrackerData";
import {
  cancelRun,
  completeRun,
  discardSession,
  endSession,
  startRun,
} from "@/lib/tracker-db";
import { getTrackerErrorMessage } from "@/lib/tracker-error";

export const Route = createFileRoute("/session")({
  component: SessionRoute,
});

function SessionRoute() {
  const navigate = useNavigate();
  const { activeSession, isLoading, loadError, refreshTracker } =
    useTrackerData();
  const [screenError, setScreenError] = useState<string | null>(null);
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!activeSession) {
      return;
    }

    const intervalId = window.setInterval(() => {
      setNow(Date.now());
    }, 100);

    return () => {
      window.clearInterval(intervalId);
    };
  }, [activeSession]);

  useEffect(() => {
    if (!isLoading && !activeSession && !busyAction) {
      void navigate({ to: "/", replace: true });
    }
  }, [activeSession, busyAction, isLoading, navigate]);

  const runAction = async (busyKey: string, action: () => Promise<void>) => {
    setBusyAction(busyKey);
    setScreenError(null);

    try {
      await action();
    } catch (error) {
      setScreenError(getTrackerErrorMessage(error));
    } finally {
      setBusyAction(null);
    }
  };

  const handleToggleRun = () => {
    if (!activeSession) {
      return;
    }

    void runAction(
      activeSession.activeRun ? "finish-run" : "start-run",
      async () => {
        if (activeSession.activeRun) {
          await completeRun(activeSession.activeRun.id, activeSession.id);
        } else {
          await startRun(activeSession.id);
        }

        await refreshTracker();
      },
    );
  };

  const handleCancelRun = () => {
    if (!activeSession?.activeRun) {
      return;
    }

    const activeRun = activeSession.activeRun;
    const sessionId = activeSession.id;

    void runAction("cancel-run", async () => {
      await cancelRun(activeRun.id, sessionId);
      await refreshTracker();
    });
  };

  const handleEndSession = () => {
    if (!activeSession) {
      return;
    }

    void runAction("end-session", async () => {
      const activeRun = activeSession.activeRun;

      if (activeRun) {
        await completeRun(activeRun.id, activeSession.id);
      }

      if (!activeRun && activeSession.completedRuns.length === 0) {
        await discardSession(activeSession.id);
      } else {
        await endSession(activeSession.id);
      }

      await refreshTracker();
      void navigate({ to: "/", replace: true });
    });
  };

  return (
    <TrackerShell error={screenError ?? loadError}>
      <ActiveSessionScreen
        activeSession={activeSession}
        busyAction={busyAction}
        now={now}
        onCancelRun={handleCancelRun}
        onEndSession={handleEndSession}
        onToggleRun={handleToggleRun}
      />
    </TrackerShell>
  );
}
