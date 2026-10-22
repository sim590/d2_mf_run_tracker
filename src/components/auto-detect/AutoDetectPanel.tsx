import type { UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

import { SectionHeader } from "@/components/tracker/SectionHeader";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  type AutoDetectStatus,
  capturePreview,
  DEFAULT_MENU_REGION,
  DEFAULT_REGION,
  findGameWindow,
  type GameWindowInfo,
  getAutoDetectStatus,
  listenRunEnd,
  listenRunStart,
  type Preview,
  type Region,
  startAutoDetect,
  stopAutoDetect,
} from "@/lib/auto-detect";

type LogEntry = {
  at: string;
  text: string;
};

export function AutoDetectPanel() {
  const [region, setRegion] = useState<Region>(DEFAULT_REGION);
  const [menuRegion, setMenuRegion] = useState<Region>(DEFAULT_MENU_REGION);
  const [menuEnabled, setMenuEnabled] = useState(true);
  const [windowInfo, setWindowInfo] = useState<GameWindowInfo | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [menuPreview, setMenuPreview] = useState<Preview | null>(null);
  const [status, setStatus] = useState<AutoDetectStatus | null>(null);
  const [log, setLog] = useState<LogEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const unlisteners: UnlistenFn[] = [];

    const register = (promise: Promise<UnlistenFn>) => {
      void promise.then((unlisten) => {
        if (cancelled) {
          unlisten();
        } else {
          unlisteners.push(unlisten);
        }
      });
    };

    register(
      listenRunStart((payload) => {
        setLog((current) =>
          [
            {
              at: new Date().toLocaleTimeString(),
              text: `run-start (${payload.source})`,
            },
            ...current,
          ].slice(0, 100),
        );
      }),
    );
    register(
      listenRunEnd((payload) => {
        const duration = (payload.duration_ms / 1000).toFixed(1);
        const short = payload.too_short ? " - too short" : "";
        setLog((current) =>
          [
            {
              at: new Date().toLocaleTimeString(),
              text: `run-end ${duration}s${short}`,
            },
            ...current,
          ].slice(0, 100),
        );
      }),
    );

    return () => {
      cancelled = true;
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  }, []);

  useEffect(() => {
    let cancelled = false;

    const refresh = async () => {
      try {
        const next = await getAutoDetectStatus();
        if (!cancelled) {
          setStatus(next);
        }
      } catch (err) {
        if (!cancelled) {
          setError(String(err));
        }
      }
    };

    void refresh();
    const interval = window.setInterval(() => {
      void refresh();
    }, 500);

    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, []);

  const appendLog = (text: string) => {
    setLog((current) =>
      [{ at: new Date().toLocaleTimeString(), text }, ...current].slice(0, 100),
    );
  };

  const runAction = async (key: string, action: () => Promise<void>) => {
    setBusy(key);
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  };

  const updateRegion = (key: keyof Region, value: number) => {
    setRegion((current) => ({ ...current, [key]: value }));
  };

  const handleFindWindow = () => {
    void runAction("find", async () => {
      const info = await findGameWindow();
      setWindowInfo(info);
      appendLog(
        `window ${info.title} ${info.width}x${info.height} at ${info.x},${info.y}`,
      );
    });
  };

  const handlePreview = () => {
    void runAction("preview", async () => {
      const result = await capturePreview(region);
      setPreview(result);
      setWindowInfo(result.window);
      appendLog(`preview brightness ${result.brightness.toFixed(1)}`);
    });
  };

  const handleMenuPreview = () => {
    void runAction("menu-preview", async () => {
      const result = await capturePreview(menuRegion);
      setMenuPreview(result);
      setWindowInfo(result.window);
      appendLog(`menu preview brightness ${result.brightness.toFixed(1)}`);
    });
  };

  const handleStart = () => {
    void runAction("start", async () => {
      await startAutoDetect({
        loading_region: region,
        menu_region: menuEnabled ? menuRegion : null,
        dark_threshold: 22,
        dark_samples: 2,
        bright_samples: 2,
        menu_threshold: 50,
        menu_samples: 2,
        poll_interval_ms: 250,
        min_run_seconds: 3,
      });
      setStatus(await getAutoDetectStatus());
      appendLog("auto-detect started");
    });
  };

  const handleStop = () => {
    void runAction("stop", async () => {
      await stopAutoDetect();
      setStatus(await getAutoDetectStatus());
      appendLog("auto-detect stopped");
    });
  };

  return (
    <div className="tracker-screen">
      <SectionHeader title="Auto-detect (experimental)" />

      <p className="tracker-copy-muted text-sm">
        Calls the Rust capture backend directly. Run events are logged here, not
        written to SQLite. The region is relative to the game window.
      </p>

      {error ? (
        <div className="border border-destructive/60 bg-destructive/10 px-4 py-3 text-sm text-destructive">
          {error}
        </div>
      ) : null}

      <section className="tracker-section-panel space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h2 className="tracker-heading text-lg">Game window</h2>
          <Button
            className="tracker-action-secondary"
            disabled={busy === "find"}
            onClick={handleFindWindow}
          >
            Find window
          </Button>
        </div>
        {windowInfo ? (
          <p className="text-sm text-muted-foreground">
            {windowInfo.title} - {windowInfo.width}x{windowInfo.height} at{" "}
            {windowInfo.x},{windowInfo.y}
          </p>
        ) : (
          <p className="text-sm text-muted-foreground">
            No game window found yet.
          </p>
        )}
      </section>

      <section className="tracker-section-panel space-y-3">
        <h2 className="tracker-heading text-lg">Loading region</h2>
        <p className="text-sm text-muted-foreground">
          A small region that is bright in game and black during a loading
          screen. The default is near the center for 2560x1440.
        </p>

        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          {(["x", "y", "width", "height"] as const).map((key) => (
            <div key={key} className="space-y-1.5">
              <Label htmlFor={`region-${key}`}>{key}</Label>
              <Input
                className="h-10 text-sm"
                id={`region-${key}`}
                min={key === "width" || key === "height" ? 1 : undefined}
                type="number"
                value={region[key]}
                onChange={(event) =>
                  updateRegion(key, Number(event.target.value))
                }
              />
            </div>
          ))}
        </div>

        <div className="flex flex-wrap items-center gap-3">
          <Button
            className="tracker-action-secondary"
            disabled={busy === "preview"}
            onClick={handlePreview}
          >
            Capture preview
          </Button>
          {preview ? (
            <span className="text-sm text-muted-foreground">
              Brightness: {preview.brightness.toFixed(1)}
            </span>
          ) : null}
        </div>

        {preview ? (
          <img
            alt="Loading region preview"
            className="max-h-64 w-full border border-border/70 bg-black object-contain"
            src={preview.image}
          />
        ) : null}
      </section>

      <section className="tracker-section-panel space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h2 className="tracker-heading text-lg">Menu region</h2>
          <label className="flex items-center gap-2 text-sm text-muted-foreground">
            <input
              checked={menuEnabled}
              className="size-4 accent-primary"
              type="checkbox"
              onChange={(event) => setMenuEnabled(event.target.checked)}
            />
            Enable menu probe
          </label>
        </div>

        <p className="text-sm text-muted-foreground">
          Bright on the character-select screen (for example the Diablo II logo)
          and dark in game. It ends the active run and suppresses false starts
          while you are in the menu.
        </p>

        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          {(["x", "y", "width", "height"] as const).map((key) => (
            <div key={key} className="space-y-1.5">
              <Label htmlFor={`menu-region-${key}`}>{key}</Label>
              <Input
                className="h-10 text-sm"
                disabled={!menuEnabled}
                id={`menu-region-${key}`}
                min={key === "width" || key === "height" ? 1 : undefined}
                type="number"
                value={menuRegion[key]}
                onChange={(event) =>
                  setMenuRegion((current) => ({
                    ...current,
                    [key]: Number(event.target.value),
                  }))
                }
              />
            </div>
          ))}
        </div>

        <div className="flex flex-wrap items-center gap-3">
          <Button
            className="tracker-action-secondary"
            disabled={!menuEnabled || busy === "menu-preview"}
            onClick={handleMenuPreview}
          >
            Capture menu preview
          </Button>
          {menuPreview ? (
            <span className="text-sm text-muted-foreground">
              Brightness: {menuPreview.brightness.toFixed(1)}
            </span>
          ) : null}
        </div>

        {menuPreview ? (
          <img
            alt="Menu region preview"
            className="max-h-64 w-full border border-border/70 bg-black object-contain"
            src={menuPreview.image}
          />
        ) : null}
      </section>

      <section className="tracker-section-panel space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h2 className="tracker-heading text-lg">Detection</h2>
          <div className="flex gap-2">
            <Button
              className="tracker-action-primary"
              disabled={busy !== null || status?.running === true}
              onClick={handleStart}
            >
              Start
            </Button>
            <Button
              className="tracker-action-secondary"
              disabled={busy !== null || status?.running !== true}
              onClick={handleStop}
            >
              Stop
            </Button>
          </div>
        </div>

        <div className="grid gap-2 text-sm text-muted-foreground sm:grid-cols-2">
          <div>Running: {status?.running ? "yes" : "no"}</div>
          <div>Phase: {status?.phase ?? "unknown"}</div>
          <div>
            Last brightness: {status?.last_brightness?.toFixed(1) ?? "none"}
          </div>
          <div>
            Menu brightness:{" "}
            {status?.last_menu_brightness?.toFixed(1) ?? "none"}
          </div>
          <div>Last event: {status?.last_event ?? "none"}</div>
          <div className="sm:col-span-2">
            Last error: {status?.last_error ?? "none"}
          </div>
        </div>
      </section>

      <section className="tracker-section-panel space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h2 className="tracker-heading text-lg">Event log</h2>
          <Button
            className="tracker-action-secondary"
            disabled={log.length === 0}
            onClick={() => setLog([])}
          >
            Clear
          </Button>
        </div>

        {log.length === 0 ? (
          <p className="text-sm text-muted-foreground">No events yet.</p>
        ) : (
          <div className="max-h-64 space-y-1 overflow-auto border border-border/40 bg-black/20 p-3 font-mono text-xs text-muted-foreground">
            {log.map((entry) => (
              <div key={`${entry.at}-${entry.text}`}>
                {entry.at} - {entry.text}
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
