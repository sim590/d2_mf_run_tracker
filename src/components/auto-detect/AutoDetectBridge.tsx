import type { UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";

import {
  listenRunEnd,
  listenRunStart,
  type RunEndPayload,
} from "@/lib/auto-detect";
import {
  completeRun,
  discardRun,
  getActiveSession,
  startRun,
} from "@/lib/tracker-db";
import {
  AUTO_DETECT_STOPPED_EVENT,
  TRACKER_REFRESH_EVENT,
} from "@/lib/tracker-events";

function notifyRefresh() {
  window.dispatchEvent(new Event(TRACKER_REFRESH_EVENT));
}

export function AutoDetectBridge() {
  const queue = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    let cancelled = false;
    const unlisteners: UnlistenFn[] = [];

    const enqueue = (task: () => Promise<void>) => {
      queue.current = queue.current.then(task).catch((error) => {
        console.error("Auto-detect database bridge:", error);
      });
    };

    const handleRunStart = async () => {
      const session = await getActiveSession();

      // The database is the source of truth: a manual run or a duplicate
      // event must not start a second run.
      if (!session || session.activeRun) {
        return;
      }

      await startRun(session.id);
      notifyRefresh();
    };

    const handleRunEnd = async (payload: RunEndPayload) => {
      const session = await getActiveSession();
      const activeRun = session?.activeRun;

      if (!session || !activeRun) {
        return;
      }

      if (payload.too_short) {
        await discardRun(activeRun.id, session.id);
      } else {
        await completeRun(activeRun.id, session.id);
      }
      notifyRefresh();
    };

    const handleDetectionStopped = async () => {
      const session = await getActiveSession();
      const activeRun = session?.activeRun;

      if (!session || !activeRun) {
        return;
      }

      await completeRun(activeRun.id, session.id);
      notifyRefresh();
    };

    const register = (promise: Promise<UnlistenFn>) => {
      void promise
        .then((unlisten) => {
          if (cancelled) {
            unlisten();
          } else {
            unlisteners.push(unlisten);
          }
        })
        .catch((error) => {
          console.error("Auto-detect bridge listener:", error);
        });
    };

    register(
      listenRunStart(() => {
        enqueue(handleRunStart);
      }),
    );
    register(
      listenRunEnd((payload) => {
        enqueue(() => handleRunEnd(payload));
      }),
    );

    const onStopped = () => {
      enqueue(handleDetectionStopped);
    };
    window.addEventListener(AUTO_DETECT_STOPPED_EVENT, onStopped);

    return () => {
      cancelled = true;
      window.removeEventListener(AUTO_DETECT_STOPPED_EVENT, onStopped);
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  }, []);

  return null;
}
