import { Link } from "@tanstack/react-router";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

import { SectionHeader } from "@/components/tracker/SectionHeader";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  type AutoDetectStatus,
  capturePreview,
  DEFAULT_HUD_REGIONS,
  DEFAULT_MENU_ANCHOR,
  DEFAULT_MENU_REGION_PERCENT,
  DEFAULT_REGION_PERCENT,
  defaultMenuAnchorRegion,
  findGameWindow,
  type GameWindowInfo,
  getAutoDetectStatus,
  HUD_MARKERS,
  HUD_REQUIRED_TEMPLATES,
  HUD_THRESHOLD,
  type HudMarkerName,
  type HudTemplateResult,
  type HudTemplateTest,
  type LiveMenuAnchor,
  listenRunEnd,
  listenRunStart,
  MENU_ANCHOR_OPTIONS,
  type MenuAnchorResult,
  type MenuTemplateTest,
  menuAnchorGroup,
  normalizeHudMarkerName,
  type Preview,
  percentToNormalized,
  type RegionPercent,
  startAutoDetect,
  stopAutoDetect,
  testHudTemplates,
  testMenuTemplates,
} from "@/lib/auto-detect";
import { type ActiveSession, getActiveSession } from "@/lib/tracker-db";
import { AUTO_DETECT_STOPPED_EVENT } from "@/lib/tracker-events";

type LogEntry = {
  at: string;
  text: string;
};

type MenuAnchorCalibration = {
  region: RegionPercent;
  text: string;
  group: string;
  font: string;
  size: number;
  letter_spacing: number;
  binary_score: number;
};

type MenuCalibration = {
  anchors: MenuAnchorCalibration[];
};

const MENU_CALIBRATION_STORAGE_KEY = "d2mf-menu-calibration-v2";
const LEGACY_MENU_CALIBRATION_STORAGE_KEY = "d2mf-menu-calibration-v1";

function isRegionPercent(value: unknown): value is RegionPercent {
  if (!value || typeof value !== "object") {
    return false;
  }
  const region = value as Partial<RegionPercent>;
  return (
    typeof region.x === "number" &&
    typeof region.y === "number" &&
    typeof region.width === "number" &&
    typeof region.height === "number"
  );
}

function parseAnchorCalibration(value: unknown): MenuAnchorCalibration | null {
  if (!value || typeof value !== "object") {
    return null;
  }
  const anchor = value as Partial<MenuAnchorCalibration>;
  if (
    typeof anchor.text !== "string" ||
    typeof anchor.group !== "string" ||
    typeof anchor.font !== "string" ||
    !isRegionPercent(anchor.region) ||
    typeof anchor.size !== "number" ||
    typeof anchor.letter_spacing !== "number" ||
    typeof anchor.binary_score !== "number"
  ) {
    return null;
  }
  return {
    region: anchor.region,
    text: anchor.text,
    group: anchor.group,
    font: anchor.font,
    size: anchor.size,
    letter_spacing: anchor.letter_spacing,
    binary_score: anchor.binary_score,
  };
}

function loadMenuCalibration(): MenuCalibration | null {
  try {
    const raw = window.localStorage.getItem(MENU_CALIBRATION_STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as { anchors?: unknown } | null;
      if (!parsed || !Array.isArray(parsed.anchors)) {
        return null;
      }
      const anchors = parsed.anchors
        .map(parseAnchorCalibration)
        .filter((anchor): anchor is MenuAnchorCalibration => anchor !== null);
      return anchors.length > 0 ? { anchors } : null;
    }

    // Migrate the old single-anchor calibration (combined score) as one left
    // anchor, so CINÉMATIQUES is not lost.
    const legacyRaw = window.localStorage.getItem(
      LEGACY_MENU_CALIBRATION_STORAGE_KEY,
    );
    if (!legacyRaw) {
      return null;
    }
    const legacy = JSON.parse(legacyRaw) as {
      region?: unknown;
      text?: unknown;
      font?: unknown;
      size?: unknown;
      letter_spacing?: unknown;
      score?: unknown;
    } | null;
    if (
      !legacy ||
      !isRegionPercent(legacy.region) ||
      typeof legacy.text !== "string" ||
      typeof legacy.font !== "string" ||
      typeof legacy.size !== "number" ||
      typeof legacy.letter_spacing !== "number"
    ) {
      return null;
    }
    const migrated: MenuCalibration = {
      anchors: [
        {
          region: legacy.region,
          text: legacy.text,
          group: "left",
          font: legacy.font,
          size: legacy.size,
          letter_spacing: legacy.letter_spacing,
          binary_score: typeof legacy.score === "number" ? legacy.score : 0,
        },
      ],
    };
    persistMenuCalibration(migrated);
    window.localStorage.removeItem(LEGACY_MENU_CALIBRATION_STORAGE_KEY);
    return migrated;
  } catch {
    return null;
  }
}

function persistMenuCalibration(calibration: MenuCalibration | null) {
  try {
    if (calibration) {
      window.localStorage.setItem(
        MENU_CALIBRATION_STORAGE_KEY,
        JSON.stringify(calibration),
      );
    } else {
      window.localStorage.removeItem(MENU_CALIBRATION_STORAGE_KEY);
      window.localStorage.removeItem(LEGACY_MENU_CALIBRATION_STORAGE_KEY);
    }
  } catch {
    // localStorage may be unavailable; the calibration still works for this
    // app session.
  }
}

type HudMarkerDraft = {
  region: RegionPercent;
  template: string | null;
  result: HudTemplateResult | null;
};

type HudCalibrationEntry = {
  name: HudMarkerName;
  region: RegionPercent;
  template: string;
  threshold: number;
  score: number;
};

type HudCalibration = {
  templates: HudCalibrationEntry[];
};

const HUD_CALIBRATION_STORAGE_KEY = "d2mf-hud-calibration-v1";

function emptyHudDrafts(): Record<HudMarkerName, HudMarkerDraft> {
  const drafts = {} as Record<HudMarkerName, HudMarkerDraft>;
  for (const marker of HUD_MARKERS) {
    drafts[marker] = {
      region: { ...DEFAULT_HUD_REGIONS[marker] },
      template: null,
      result: null,
    };
  }
  return drafts;
}

function parseHudCalibrationEntry(value: unknown): HudCalibrationEntry | null {
  if (!value || typeof value !== "object") {
    return null;
  }
  const entry = value as Partial<HudCalibrationEntry>;
  const name =
    typeof entry.name === "string" ? normalizeHudMarkerName(entry.name) : null;
  if (
    !name ||
    !isRegionPercent(entry.region) ||
    typeof entry.template !== "string" ||
    entry.template.length === 0 ||
    typeof entry.threshold !== "number" ||
    typeof entry.score !== "number"
  ) {
    return null;
  }
  return {
    name,
    region: entry.region,
    template: entry.template,
    threshold: entry.threshold,
    score: entry.score,
  };
}

function loadHudCalibration(): HudCalibration | null {
  try {
    const raw = window.localStorage.getItem(HUD_CALIBRATION_STORAGE_KEY);
    if (!raw) {
      return null;
    }
    const parsed = JSON.parse(raw) as { templates?: unknown } | null;
    if (!parsed || !Array.isArray(parsed.templates)) {
      return null;
    }
    const templates = parsed.templates
      .map(parseHudCalibrationEntry)
      .filter((entry): entry is HudCalibrationEntry => entry !== null);
    return templates.length > 0 ? { templates } : null;
  } catch {
    return null;
  }
}

function persistHudCalibration(calibration: HudCalibration | null) {
  try {
    if (calibration) {
      window.localStorage.setItem(
        HUD_CALIBRATION_STORAGE_KEY,
        JSON.stringify(calibration),
      );
    } else {
      window.localStorage.removeItem(HUD_CALIBRATION_STORAGE_KEY);
    }
  } catch {
    // localStorage may be unavailable; the calibration still works for this
    // app session.
  }
}

function hudDraftsFromCalibration(
  calibration: HudCalibration | null,
): Record<HudMarkerName, HudMarkerDraft> {
  const drafts = emptyHudDrafts();
  for (const entry of calibration?.templates ?? []) {
    drafts[entry.name] = {
      region: { ...entry.region },
      template: entry.template,
      result: null,
    };
  }
  return drafts;
}

function isValidAnchorResult(result: MenuAnchorResult): boolean {
  return result.size > 0 && result.binary_score >= 0.3;
}

function buildAnchorCalibration(
  result: MenuAnchorResult,
  region: RegionPercent,
): MenuAnchorCalibration {
  return {
    region,
    text: result.text,
    group: result.group || menuAnchorGroup(result.text) || "left",
    font: result.font,
    size: result.size,
    letter_spacing: result.letter_spacing,
    binary_score: result.binary_score,
  };
}

function mergeAnchorCalibration(
  current: MenuCalibration | null,
  additions: MenuAnchorCalibration[],
): MenuCalibration {
  const anchors = [...(current?.anchors ?? [])];
  for (const addition of additions) {
    const index = anchors.findIndex((anchor) => anchor.text === addition.text);
    if (index >= 0) {
      anchors[index] = addition;
    } else {
      anchors.push(addition);
    }
  }
  return { anchors };
}

function menuAnchorsVisible(anchors: LiveMenuAnchor[]): boolean {
  const groups = ["left", "center", "right"] as const;
  return groups.every((group) =>
    anchors.some((anchor) => anchor.group === group && anchor.visible),
  );
}

function formatLiveAnchors(anchors: LiveMenuAnchor[]): string {
  if (anchors.length === 0) {
    return "none";
  }

  const groups = ["left", "center", "right"] as const;
  const parts = groups.map((group) => {
    const groupAnchors = anchors.filter((anchor) => anchor.group === group);
    if (groupAnchors.length === 0) {
      return `${group} missing`;
    }
    const best = groupAnchors.reduce((current, entry) =>
      entry.score > current.score ? entry : current,
    );
    return `${group} ${best.score.toFixed(2)} (${best.text})`;
  });
  const menuVisible = groups.every((group) =>
    anchors.some((anchor) => anchor.group === group && anchor.visible),
  );
  return `${parts.join(", ")} -> ${menuVisible ? "menu" : "not menu"}`;
}

export function AutoDetectPanel() {
  const [region, setRegion] = useState<RegionPercent>(DEFAULT_REGION_PERCENT);
  const [menuRegion, setMenuRegion] = useState<RegionPercent>(
    DEFAULT_MENU_REGION_PERCENT,
  );
  const [menuEnabled, setMenuEnabled] = useState(true);
  const [menuAnchorRegion, setMenuAnchorRegion] = useState<RegionPercent>(() =>
    defaultMenuAnchorRegion(DEFAULT_MENU_ANCHOR),
  );
  const [menuAnchor, setMenuAnchor] = useState<string>(DEFAULT_MENU_ANCHOR);
  const [windowInfo, setWindowInfo] = useState<GameWindowInfo | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [menuPreview, setMenuPreview] = useState<Preview | null>(null);
  const [menuTemplateTest, setMenuTemplateTest] =
    useState<MenuTemplateTest | null>(null);
  const [menuCalibration, setMenuCalibration] =
    useState<MenuCalibration | null>(() => loadMenuCalibration());
  const [hudCalibration, setHudCalibration] = useState<HudCalibration | null>(
    () => loadHudCalibration(),
  );
  const [hudDrafts, setHudDrafts] = useState<
    Record<HudMarkerName, HudMarkerDraft>
  >(() => hudDraftsFromCalibration(loadHudCalibration()));
  const [hudTest, setHudTest] = useState<HudTemplateTest | null>(null);
  const [status, setStatus] = useState<AutoDetectStatus | null>(null);
  const [activeSession, setActiveSession] = useState<ActiveSession | null>(
    null,
  );
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
        const [nextStatus, nextSession] = await Promise.all([
          getAutoDetectStatus(),
          getActiveSession(),
        ]);
        if (!cancelled) {
          setStatus(nextStatus);
          setActiveSession(nextSession);
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

  const updateRegion = (key: keyof RegionPercent, value: number) => {
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
      const result = await capturePreview(percentToNormalized(region));
      setPreview(result);
      setWindowInfo(result.window);
      appendLog(`preview brightness ${result.brightness.toFixed(1)}`);
    });
  };

  const handleMenuPreview = () => {
    void runAction("menu-preview", async () => {
      const result = await capturePreview(percentToNormalized(menuRegion));
      setMenuPreview(result);
      setWindowInfo(result.window);
      appendLog(`menu preview brightness ${result.brightness.toFixed(1)}`);
    });
  };

  const updateMenuAnchorRegion = (key: keyof RegionPercent, value: number) => {
    setMenuAnchorRegion((current) => ({ ...current, [key]: value }));
  };

  const savedAnchorFor = (text: string) =>
    menuCalibration?.anchors.find((anchor) => anchor.text === text) ?? null;

  const regionForAnchor = (text: string): RegionPercent =>
    savedAnchorFor(text)?.region ?? defaultMenuAnchorRegion(text);

  const handleMenuAnchorChange = (value: string) => {
    setMenuAnchor(value);
    setMenuTemplateTest(null);
    if (value === "all") {
      return;
    }
    setMenuAnchorRegion(regionForAnchor(value));
  };

  const handleTestMenuTemplates = () => {
    void runAction("test-menu-templates", async () => {
      if (menuAnchor === "all") {
        const results: MenuAnchorResult[] = [];
        let window: GameWindowInfo | null = null;

        for (const anchorText of MENU_ANCHOR_OPTIONS) {
          const region = regionForAnchor(anchorText);
          const result = await testMenuTemplates(
            percentToNormalized(region),
            anchorText,
          );
          results.push(...result.results);
          window = result.window;
        }

        if (!window) {
          return;
        }
        setMenuTemplateTest({ window, preview: "", results });
        setWindowInfo(window);

        const valid = results.filter(isValidAnchorResult);
        const best =
          valid.length > 0
            ? valid.reduce((current, entry) =>
                entry.binary_score > current.binary_score ? entry : current,
              )
            : null;
        appendLog(
          best
            ? `all anchors: ${valid.length}/${MENU_ANCHOR_OPTIONS.length} valid, best ${best.text} binary ${best.binary_score.toFixed(3)}`
            : `all anchors: 0/${MENU_ANCHOR_OPTIONS.length} valid`,
        );
        return;
      }

      const result = await testMenuTemplates(
        percentToNormalized(menuAnchorRegion),
        menuAnchor,
      );
      setMenuTemplateTest(result);
      setWindowInfo(result.window);
      const best = result.results[0];
      if (best) {
        appendLog(
          `${best.text} binary ${best.binary_score.toFixed(3)} (${best.font} ${best.size}px, ${best.letter_spacing}px spacing) at ${best.x},${best.y}`,
        );
      }
    });
  };

  const handleSaveMenuAnchor = () => {
    if (menuAnchor === "all") {
      return;
    }
    const result = menuTemplateTest?.results[0];
    if (!result || !isValidAnchorResult(result)) {
      return;
    }

    const calibration = mergeAnchorCalibration(menuCalibration, [
      buildAnchorCalibration(result, menuAnchorRegion),
    ]);
    setMenuCalibration(calibration);
    persistMenuCalibration(calibration);
    appendLog(
      `menu calibration saved: ${result.text} ${result.font} ${result.size}px, binary ${result.binary_score.toFixed(3)}`,
    );
  };

  const handleSaveAllMenuAnchors = () => {
    const results = menuTemplateTest?.results ?? [];
    const valid = results.filter(isValidAnchorResult);
    if (valid.length === 0) {
      setError("No anchor had a size and a binary score of at least 0.300.");
      return;
    }

    const additions = valid.map((result) =>
      buildAnchorCalibration(result, regionForAnchor(result.text)),
    );
    const calibration = mergeAnchorCalibration(menuCalibration, additions);
    setMenuCalibration(calibration);
    persistMenuCalibration(calibration);
    appendLog(
      `menu calibration saved: ${additions
        .map(
          (anchor) =>
            `${anchor.group} ${anchor.text} ${anchor.binary_score.toFixed(3)}`,
        )
        .join(", ")}`,
    );
  };

  const handleClearMenuCalibration = () => {
    setMenuCalibration(null);
    persistMenuCalibration(null);
    appendLog("menu calibration cleared");
  };

  const updateHudRegion = (
    marker: HudMarkerName,
    key: keyof RegionPercent,
    value: number,
  ) => {
    setHudDrafts((current) => ({
      ...current,
      [marker]: {
        ...current[marker],
        region: { ...current[marker].region, [key]: value },
        // The captured template belongs to the previous region.
        template: null,
        result: null,
      },
    }));
  };

  const handleHudCapture = (marker: HudMarkerName) => {
    void runAction(`hud-capture-${marker}`, async () => {
      const draft = hudDrafts[marker];
      const preview = await capturePreview(percentToNormalized(draft.region));
      setWindowInfo(preview.window);
      setHudDrafts((current) => ({
        ...current,
        [marker]: {
          ...current[marker],
          template: preview.image,
          result: null,
        },
      }));
      appendLog(
        `${marker} template captured (${preview.window.width}x${preview.window.height})`,
      );
    });
  };

  const handleTestHudTemplates = () => {
    void runAction("test-hud-templates", async () => {
      const inputs = HUD_MARKERS.flatMap((marker) => {
        const draft = hudDrafts[marker];
        if (!draft.template) {
          return [];
        }
        return [
          {
            name: marker,
            region: percentToNormalized(draft.region),
            template: draft.template,
            threshold: HUD_THRESHOLD,
          },
        ];
      });

      if (inputs.length < HUD_REQUIRED_TEMPLATES) {
        setError(
          `Capture at least ${HUD_REQUIRED_TEMPLATES} HUD templates before testing.`,
        );
        return;
      }

      const result = await testHudTemplates(inputs);
      setHudTest(result);
      setWindowInfo(result.window);
      setHudDrafts((current) => {
        const next = { ...current };
        for (const entry of result.results) {
          const marker = normalizeHudMarkerName(entry.name);
          if (marker) {
            next[marker] = { ...next[marker], result: entry };
          }
        }
        return next;
      });
      appendLog(
        `HUD test: ${result.results.filter((entry) => entry.visible).length}/${result.results.length} visible -> ${result.hud_visible ? "in-game" : "not in-game"}`,
      );
    });
  };

  const handleSaveHudCalibration = () => {
    const captured = HUD_MARKERS.flatMap((marker) => {
      const draft = hudDrafts[marker];
      if (!draft.template) {
        return [];
      }
      return [
        {
          name: marker,
          region: draft.region,
          template: draft.template,
          threshold: HUD_THRESHOLD,
          score: draft.result?.score ?? 0,
        },
      ];
    });

    if (captured.length < HUD_REQUIRED_TEMPLATES) {
      setError(
        `Capture at least ${HUD_REQUIRED_TEMPLATES} HUD templates before saving.`,
      );
      return;
    }

    // Merge by name so the user can save markers one at a time.
    const templates = [...(hudCalibration?.templates ?? [])];
    for (const entry of captured) {
      const index = templates.findIndex(
        (template) => template.name === entry.name,
      );
      if (index >= 0) {
        templates[index] = entry;
      } else {
        templates.push(entry);
      }
    }

    const calibration: HudCalibration = { templates };
    setHudCalibration(calibration);
    persistHudCalibration(calibration);
    appendLog(
      `HUD calibration saved: ${captured
        .map((entry) => `${entry.name} ${entry.score.toFixed(3)}`)
        .join(", ")}`,
    );
  };

  const handleClearHudCalibration = () => {
    setHudCalibration(null);
    persistHudCalibration(null);
    setHudDrafts(emptyHudDrafts());
    setHudTest(null);
    appendLog("HUD calibration cleared");
  };

  const handleStart = () => {
    if (!activeSession) {
      setError("Create a session before enabling auto-detect.");
      return;
    }

    const calibrationAnchors = menuCalibration?.anchors ?? [];
    const hasGroup = (group: string) =>
      calibrationAnchors.some((anchor) => anchor.group === group);
    const templateMenuReady =
      hasGroup("left") && hasGroup("center") && hasGroup("right");
    // Every saved anchor is sent, including CHARGEMENT. The menu rule still
    // needs left+center+right, but the loading anchor alone can feed the
    // loading guard in the HUD-only fallback.
    const menuTemplates = calibrationAnchors.map((anchor) => ({
      region: percentToNormalized(anchor.region),
      text: anchor.text,
      group: anchor.group,
      font: anchor.font,
      size: anchor.size,
      letter_spacing: anchor.letter_spacing,
      threshold: 0.3,
    }));

    const savedHudTemplates = hudCalibration?.templates ?? [];
    const hudTemplates =
      savedHudTemplates.length >= HUD_REQUIRED_TEMPLATES
        ? savedHudTemplates.map((template) => ({
            name: template.name,
            region: percentToNormalized(template.region),
            template: template.template,
            threshold: HUD_THRESHOLD,
          }))
        : [];

    void runAction("start", async () => {
      await startAutoDetect({
        loading_region: percentToNormalized(region),
        menu_region: menuEnabled ? percentToNormalized(menuRegion) : null,
        menu_templates: menuTemplates,
        hud_templates: hudTemplates,
        dark_threshold: 22,
        dark_samples: 2,
        bright_samples: 2,
        menu_threshold: 50,
        menu_samples: 2,
        menu_dark_samples: templateMenuReady ? 2 : 4,
        hud_samples: 2,
        loading_samples: 1,
        poll_interval_ms: 125,
        min_run_seconds: 3,
      });
      setStatus(await getAutoDetectStatus());
      appendLog(
        hudTemplates.length > 0
          ? `auto-detect started (${templateMenuReady ? "combined menu + HUD" : "HUD only"})`
          : "auto-detect started (menu/brightness only)",
      );
    });
  };

  const handleStop = () => {
    void runAction("stop", async () => {
      await stopAutoDetect();
      window.dispatchEvent(new Event(AUTO_DETECT_STOPPED_EVENT));
      setStatus(await getAutoDetectStatus());
      appendLog("auto-detect stopped");
    });
  };

  const bestMenuBinaryScore =
    menuTemplateTest && menuTemplateTest.results.length > 0
      ? Math.max(
          ...menuTemplateTest.results.map((result) => result.binary_score),
        )
      : 0;

  const savedAnchors = menuCalibration?.anchors ?? [];
  const savedGroups = new Set(savedAnchors.map((anchor) => anchor.group));
  const missingGroups = (["left", "center", "right"] as const).filter(
    (group) => !savedGroups.has(group),
  );
  const loadingAnchorSaved =
    savedAnchors.find((anchor) => anchor.group === "loading") ?? null;
  const templateMenuReady = missingGroups.length === 0;

  const singleResult =
    menuAnchor === "all" ? null : (menuTemplateTest?.results[0] ?? null);
  const canSaveSingle =
    singleResult !== null && isValidAnchorResult(singleResult);
  const validAllResults =
    menuAnchor === "all"
      ? (menuTemplateTest?.results ?? []).filter(isValidAnchorResult)
      : [];
  const canSaveAll = validAllResults.length > 0;

  const savedHudTemplates = hudCalibration?.templates ?? [];
  const hudCombinedReady = savedHudTemplates.length >= HUD_REQUIRED_TEMPLATES;
  const capturedHudCount = HUD_MARKERS.filter(
    (marker) => hudDrafts[marker].template !== null,
  ).length;
  const hudScores = status?.last_hud_scores ?? [];
  const hudVisibleCount = hudScores.filter((entry) => entry.visible).length;
  const hudFrameVisible = status?.last_hud_visible ?? false;
  const menuFrameVisible = menuAnchorsVisible(status?.last_menu_anchors ?? []);
  const loadingVisible = status?.last_loading_visible ?? false;
  const liveLoadingAnchor =
    (status?.last_menu_anchors ?? []).find(
      (anchor) => anchor.group === "loading",
    ) ?? null;
  const combinedSignal = loadingVisible
    ? "loading (phase kept)"
    : hudFrameVisible
      ? "in-game (HUD)"
      : menuFrameVisible
        ? "menu"
        : "transition (phase kept)";
  const startMode = hudCombinedReady
    ? templateMenuReady
      ? "Combined menu + HUD"
      : "HUD only (menu calibration incomplete)"
    : templateMenuReady
      ? "Menu only (HUD calibration incomplete)"
      : "Brightness fallback (no template calibration)";

  return (
    <div className="tracker-screen">
      <SectionHeader title="Auto-detect (experimental)" />

      <p className="tracker-copy-muted text-sm">
        Calls the Rust capture backend directly. Run events are applied to the
        active session's database when one is open; otherwise they are only
        logged here. Regions are percentages of the game window.
      </p>

      {activeSession ? (
        <p className="text-sm text-muted-foreground">
          Active session: {activeSession.locationName}
        </p>
      ) : (
        <div className="border border-amber-500/60 bg-amber-500/10 px-4 py-3 text-sm text-amber-300">
          <p>
            No active session. Create a session first; auto-detect will start
            the first run when you enter a game. Until then, run events are
            logged here but are not written to the database.
          </p>
          <Button asChild className="tracker-action-secondary mt-2">
            <Link to="/new-session">Start a session</Link>
          </Button>
        </div>
      )}

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
          A small region that is bright in game. It is usually black when you
          leave a game to the menu, but the enter-game loading animation can be
          bright (the opening portal). The menu probe handles that transition.
          Values are percentages of the game window; the defaults match
          2560x1440.
        </p>

        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          {(["x", "y", "width", "height"] as const).map((key) => (
            <div key={key} className="space-y-1.5">
              <Label htmlFor={`region-${key}`}>{key} (%)</Label>
              <Input
                className="h-10 text-sm"
                id={`region-${key}`}
                max={100}
                min={key === "width" || key === "height" ? 0.1 : 0}
                step={0.1}
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
          and dark in game. It is only used when no menu or HUD template is
          calibrated. It ends the active run when you return to the menu, and it
          starts a new run when you leave the menu. Loading screens inside the
          game do not end the run. Values are percentages of the game window.
        </p>

        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          {(["x", "y", "width", "height"] as const).map((key) => (
            <div key={key} className="space-y-1.5">
              <Label htmlFor={`menu-region-${key}`}>{key} (%)</Label>
              <Input
                className="h-10 text-sm"
                disabled={!menuEnabled}
                id={`menu-region-${key}`}
                max={100}
                min={key === "width" || key === "height" ? 0.1 : 0}
                step={0.1}
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
          <h2 className="tracker-heading text-lg">Menu template (run end)</h2>
          <span className="border border-border/70 px-2 py-0.5 text-xs uppercase tracking-wide text-muted-foreground">
            Live run end
          </span>
        </div>

        <p className="text-sm text-muted-foreground">
          The menu template is the fast run-end trigger. Crop a single anchor
          text at a time (no neighbouring buttons). Each anchor has its own
          default region at 2560x1440 and its own saved region once calibrated.
          “All anchors” tests the six anchors sequentially in their own regions,
          not all in one crop. Live detection needs at least one visible left,
          one center and one right anchor, and the HUD must not be visible. The
          CHARGEMENT anchor is optional for the menu rule: it is the loading
          guard, which keeps the phase and blocks menu false positives while a
          level loads. Values are percentages of the game window.
        </p>

        <p className="text-xs text-muted-foreground">
          CHARGEMENT is calibrated on a loading screen, not on the character
          select. The All anchors run at the character select cannot save it:
          select CHARGEMENT while a level loads, test it, then save this anchor.
        </p>

        <div className="space-y-1.5">
          <Label htmlFor="menu-anchor-select">Anchor</Label>
          <select
            className="h-10 w-full border border-input bg-background/80 px-3 text-sm text-foreground outline-none focus-visible:border-ring"
            id="menu-anchor-select"
            value={menuAnchor}
            onChange={(event) => handleMenuAnchorChange(event.target.value)}
          >
            {MENU_ANCHOR_OPTIONS.map((option) => (
              <option key={option} value={option}>
                {option}
              </option>
            ))}
            <option value="all">All anchors</option>
          </select>
        </div>

        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          {(["x", "y", "width", "height"] as const).map((key) => (
            <div key={key} className="space-y-1.5">
              <Label htmlFor={`menu-anchor-region-${key}`}>{key} (%)</Label>
              <Input
                className="h-10 text-sm"
                disabled={menuAnchor === "all"}
                id={`menu-anchor-region-${key}`}
                max={100}
                min={key === "width" || key === "height" ? 0.1 : 0}
                step={0.1}
                type="number"
                value={menuAnchorRegion[key]}
                onChange={(event) =>
                  updateMenuAnchorRegion(key, Number(event.target.value))
                }
              />
            </div>
          ))}
        </div>

        <div className="flex flex-wrap items-center gap-3">
          <Button
            className="tracker-action-secondary"
            disabled={busy === "test-menu-templates"}
            onClick={handleTestMenuTemplates}
          >
            {busy === "test-menu-templates"
              ? "Testing..."
              : "Test menu templates"}
          </Button>
          {menuTemplateTest ? (
            <span className="text-sm text-muted-foreground">
              Best binary score: {bestMenuBinaryScore.toFixed(3)}
            </span>
          ) : null}
        </div>

        <p className="text-xs text-muted-foreground">
          Testing alone does not change run detection. Saving anchors switches
          the menu boundary to the binary template; the brightness menu probe
          stays as a fallback. Saving CHARGEMENT enables the loading guard.
        </p>

        {menuTemplateTest ? (
          <div className="space-y-3">
            <div className="grid gap-2 sm:grid-cols-3">
              {menuTemplateTest.results.map((result) => {
                const isBest =
                  result.binary_score > 0 &&
                  result.binary_score === bestMenuBinaryScore;
                return (
                  <div
                    key={result.text}
                    className={
                      isBest
                        ? "space-y-1 border border-emerald-500/70 bg-emerald-500/10 p-3"
                        : "space-y-1 border border-border/50 p-3"
                    }
                  >
                    <div className="flex items-baseline justify-between gap-2">
                      <span className="text-sm font-medium">{result.text}</span>
                      <span
                        className={
                          isBest
                            ? "font-mono text-sm text-emerald-400"
                            : "font-mono text-sm text-muted-foreground"
                        }
                      >
                        {result.size > 0
                          ? `combined ${result.score.toFixed(3)}`
                          : ""}
                      </span>
                    </div>
                    {result.size > 0 ? (
                      <>
                        <div className="text-xs text-muted-foreground">
                          {result.group || "unknown"} - {result.font} font,{" "}
                          {result.size}px, {result.letter_spacing}px spacing
                        </div>
                        <div className="grid grid-cols-3 gap-1 font-mono text-xs text-muted-foreground">
                          <span>ncc {result.ncc_score.toFixed(3)}</span>
                          <span>bin {result.binary_score.toFixed(3)}</span>
                          <span>edge {result.edge_score.toFixed(3)}</span>
                        </div>
                        <div className="text-xs text-muted-foreground">
                          at {result.x},{result.y} region-relative (
                          {result.width}x{result.height})
                        </div>
                      </>
                    ) : (
                      <div className="text-xs text-muted-foreground">
                        region too small for any template
                      </div>
                    )}
                  </div>
                );
              })}
            </div>

            {menuTemplateTest.preview ? (
              <img
                alt="Menu template test preview"
                className="max-h-64 w-full border border-border/70 bg-black object-contain"
                src={menuTemplateTest.preview}
              />
            ) : null}

            <div className="flex flex-wrap items-center gap-3">
              {menuAnchor === "all" ? (
                <>
                  <Button
                    className="tracker-action-primary"
                    disabled={!canSaveAll}
                    onClick={handleSaveAllMenuAnchors}
                  >
                    Save all valid anchors
                  </Button>
                  <span className="text-xs text-muted-foreground">
                    {canSaveAll
                      ? `${validAllResults.length}/${MENU_ANCHOR_OPTIONS.length} anchors have a size and a binary score of at least 0.300.`
                      : "No anchor has a size and a binary score of at least 0.300."}
                  </span>
                </>
              ) : (
                <>
                  <Button
                    className="tracker-action-primary"
                    disabled={!canSaveSingle}
                    onClick={handleSaveMenuAnchor}
                  >
                    Save this anchor
                  </Button>
                  <span className="text-xs text-muted-foreground">
                    {canSaveSingle && singleResult
                      ? `Saves ${singleResult.text} (${singleResult.group}) with binary ${singleResult.binary_score.toFixed(3)}.`
                      : "Need a size and a binary score of at least 0.300."}
                  </span>
                </>
              )}
            </div>

            {menuAnchor === "all" ? (
              <p className="text-xs text-muted-foreground">
                Each anchor was tested in its own region. Run a single anchor to
                see its preview.
              </p>
            ) : null}
          </div>
        ) : null}

        <div
          className={
            menuCalibration
              ? "space-y-1 border border-emerald-500/60 bg-emerald-500/10 p-3"
              : "space-y-1 border border-border/50 p-3"
          }
        >
          <div className="flex flex-wrap items-center justify-between gap-3">
            <span className="text-sm font-medium">
              {menuCalibration
                ? "Saved menu calibration"
                : "No saved menu calibration"}
            </span>
            {menuCalibration ? (
              <Button
                className="tracker-action-secondary"
                onClick={handleClearMenuCalibration}
              >
                Clear calibration
              </Button>
            ) : null}
          </div>
          {menuCalibration ? (
            <>
              {(["left", "center", "right"] as const).map((group) => {
                const entries = savedAnchors.filter(
                  (anchor) => anchor.group === group,
                );
                return (
                  <div key={group} className="text-xs text-muted-foreground">
                    <span className="font-mono uppercase">{group}</span>:{" "}
                    {entries.length > 0
                      ? entries
                          .map(
                            (anchor) =>
                              `${anchor.text} binary ${anchor.binary_score.toFixed(3)} (${anchor.font} ${anchor.size}px, ${anchor.letter_spacing}px spacing) @ ${anchor.region.x}/${anchor.region.y}/${anchor.region.width}x${anchor.region.height}%`,
                          )
                          .join(" | ")
                      : "missing"}
                  </div>
                );
              })}
              <div className="text-xs text-muted-foreground">
                <span className="font-mono uppercase">loading</span>:{" "}
                {loadingAnchorSaved
                  ? `${loadingAnchorSaved.text} binary ${loadingAnchorSaved.binary_score.toFixed(3)} (${loadingAnchorSaved.font} ${loadingAnchorSaved.size}px, ${loadingAnchorSaved.letter_spacing}px spacing) @ ${loadingAnchorSaved.region.x}/${loadingAnchorSaved.region.y}/${loadingAnchorSaved.region.width}x${loadingAnchorSaved.region.height}%`
                  : "missing (optional for the menu rule)"}
              </div>
              <p className="text-xs text-muted-foreground">
                {loadingAnchorSaved
                  ? "The loading anchor feeds the loading guard: its one-frame signal keeps the phase and suppresses menu false-positives while a level loads."
                  : "No loading guard; loading false-positives may end runs. Capture CHARGEMENT while a loading screen is visible, then save it."}
              </p>
              <p className="text-xs text-muted-foreground">
                {templateMenuReady
                  ? "Template menu detection is active: at least one visible anchor in each group is required. Live threshold: binary 0.300 over two frames."
                  : `Template menu detection needs one left, one center and one right anchor. Missing: ${missingGroups.join(", ")}. The brightness menu probe is used meanwhile.`}
              </p>
            </>
          ) : (
            <p className="text-xs text-muted-foreground">
              Without a calibration, the brightness menu region is used. Save at
              least one anchor in each of the left, center and right groups to
              switch the menu boundary to the binary template.
            </p>
          )}
        </div>
      </section>

      <section className="tracker-section-panel space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h2 className="tracker-heading text-lg">HUD template calibration</h2>
          <span className="border border-border/70 px-2 py-0.5 text-xs uppercase tracking-wide text-muted-foreground">
            Live signal
          </span>
        </div>

        <p className="text-sm text-muted-foreground">
          Capture the static life-orb frame, mana-orb frame and skill-bar frame
          from the game (the orb numbers can be disabled; the frames stay). At
          least two markers are required. The HUD confirms you are in game and
          blocks the menu template from ending a run on an inventory tooltip.
          Regions are percentages of the game window; the defaults match
          2560x1440. Changing a region invalidates its captured template, so
          capture again.
        </p>

        {HUD_MARKERS.map((marker) => {
          const draft = hudDrafts[marker];
          return (
            <div key={marker} className="space-y-3 border border-border/50 p-3">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <span className="text-sm font-medium capitalize">{marker}</span>
                {draft.result ? (
                  <span
                    className={
                      draft.result.visible
                        ? "font-mono text-sm text-emerald-400"
                        : "font-mono text-sm text-muted-foreground"
                    }
                  >
                    {draft.result.visible ? "visible" : "not visible"} - score{" "}
                    {draft.result.score.toFixed(3)}
                  </span>
                ) : draft.template ? (
                  <span className="text-xs text-muted-foreground">
                    template captured, not tested
                  </span>
                ) : null}
              </div>

              <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
                {(["x", "y", "width", "height"] as const).map((key) => (
                  <div key={key} className="space-y-1.5">
                    <Label htmlFor={`hud-${marker}-${key}`}>{key} (%)</Label>
                    <Input
                      className="h-10 text-sm"
                      id={`hud-${marker}-${key}`}
                      max={100}
                      min={key === "width" || key === "height" ? 0.1 : 0}
                      step={0.1}
                      type="number"
                      value={draft.region[key]}
                      onChange={(event) =>
                        updateHudRegion(marker, key, Number(event.target.value))
                      }
                    />
                  </div>
                ))}
              </div>

              <div className="flex flex-wrap items-center gap-3">
                <Button
                  className="tracker-action-secondary"
                  disabled={busy === `hud-capture-${marker}`}
                  onClick={() => handleHudCapture(marker)}
                >
                  Capture template
                </Button>
                {draft.template ? (
                  <span className="text-xs text-muted-foreground">
                    Template captured. Adjust the region and capture again if
                    the frame does not fill it.
                  </span>
                ) : null}
              </div>

              {draft.template ? (
                <img
                  alt={`${marker} HUD template`}
                  className="max-h-40 w-full border border-border/70 bg-black object-contain"
                  src={draft.template}
                />
              ) : null}

              {draft.result ? (
                <div className="grid grid-cols-3 gap-1 font-mono text-xs text-muted-foreground">
                  <span>ncc {draft.result.ncc_score.toFixed(3)}</span>
                  <span>bin {draft.result.binary_score.toFixed(3)}</span>
                  <span>edge {draft.result.edge_score.toFixed(3)}</span>
                </div>
              ) : null}
            </div>
          );
        })}

        <div className="flex flex-wrap items-center gap-3">
          <Button
            className="tracker-action-secondary"
            disabled={busy === "test-hud-templates"}
            onClick={handleTestHudTemplates}
          >
            {busy === "test-hud-templates"
              ? "Testing..."
              : "Test HUD templates"}
          </Button>
          <Button
            className="tracker-action-primary"
            disabled={capturedHudCount < HUD_REQUIRED_TEMPLATES}
            onClick={handleSaveHudCalibration}
          >
            Save HUD calibration
          </Button>
          {hudTest ? (
            <span
              className={
                hudTest.hud_visible
                  ? "text-sm text-emerald-400"
                  : "text-sm text-muted-foreground"
              }
            >
              {hudTest.results.filter((entry) => entry.visible).length}/
              {hudTest.results.length} visible -&gt;{" "}
              {hudTest.hud_visible ? "in-game" : "not in-game"}
            </span>
          ) : null}
        </div>

        <p className="text-xs text-muted-foreground">
          The live decision uses max(edge, ncc) with a fixed 0.300 threshold.
          The binary score is diagnostic only. Testing alone does not change run
          detection.
        </p>

        <div
          className={
            hudCalibration
              ? "space-y-1 border border-emerald-500/60 bg-emerald-500/10 p-3"
              : "space-y-1 border border-border/50 p-3"
          }
        >
          <div className="flex flex-wrap items-center justify-between gap-3">
            <span className="text-sm font-medium">
              {hudCalibration
                ? "Saved HUD calibration"
                : "No saved HUD calibration"}
            </span>
            {hudCalibration ? (
              <Button
                className="tracker-action-secondary"
                onClick={handleClearHudCalibration}
              >
                Clear
              </Button>
            ) : null}
          </div>
          {hudCalibration ? (
            <>
              <div className="text-xs text-muted-foreground">
                {savedHudTemplates
                  .map(
                    (template) =>
                      `${template.name} ${template.score.toFixed(3)} @ ${template.region.x}/${template.region.y}/${template.region.width}x${template.region.height}%`,
                  )
                  .join(" | ")}
              </div>
              <p className="text-xs text-muted-foreground">
                {hudCombinedReady
                  ? "Combined detection is available: the menu ends runs fast and the HUD keeps the detector in game during inventory, tooltips and teleports."
                  : `Combined detection needs at least ${HUD_REQUIRED_TEMPLATES} saved markers. The menu-only detector is used meanwhile.`}
              </p>
            </>
          ) : (
            <p className="text-xs text-muted-foreground">
              Without HUD calibration the menu-only detector runs. Capture at
              least {HUD_REQUIRED_TEMPLATES} markers to enable the combined
              detector.
            </p>
          )}
        </div>

        <p className="text-xs text-muted-foreground">
          Resolution or UI-scale changes require recalibration: the template
          pixels no longer line up with the captured region.
        </p>
      </section>

      <section className="tracker-section-panel space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h2 className="tracker-heading text-lg">Detection</h2>
          <div className="flex gap-2">
            <Button
              className="tracker-action-primary"
              disabled={
                busy !== null || status?.running === true || !activeSession
              }
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

        <p className="text-xs text-muted-foreground">Start mode: {startMode}</p>

        <div className="grid gap-2 text-sm text-muted-foreground sm:grid-cols-2">
          <div>Running: {status?.running ? "yes" : "no"}</div>
          <div>Phase: {status?.phase ?? "unknown"}</div>
          <div>Combined signal: {combinedSignal}</div>
          <div>
            Loading:{" "}
            {loadingAnchorSaved === null
              ? "not calibrated"
              : loadingVisible
                ? `visible -> transition${liveLoadingAnchor ? ` (binary ${liveLoadingAnchor.score.toFixed(3)})` : ""}`
                : "none"}
          </div>
          <div>Last event: {status?.last_event ?? "none"}</div>
          <div>
            Menu template score: {status?.last_menu_score?.toFixed(3) ?? "none"}
          </div>
          <div>
            HUD:{" "}
            {hudScores.length > 0
              ? `${hudVisibleCount}/${hudScores.length} -> ${hudFrameVisible ? "in-game" : "not in-game"}`
              : "not calibrated"}
          </div>
          <div className="sm:col-span-2">
            Menu anchors: {formatLiveAnchors(status?.last_menu_anchors ?? [])}
          </div>
          <div className="sm:col-span-2">
            HUD scores:{" "}
            {hudScores.length > 0
              ? hudScores
                  .map(
                    (entry) =>
                      `${entry.name} ${entry.score.toFixed(3)}${entry.visible ? "" : " (off)"}`,
                  )
                  .join(", ")
              : "none"}
          </div>
          <div className="sm:col-span-2">
            Last brightness: {status?.last_brightness?.toFixed(1) ?? "none"}
          </div>
          <div className="sm:col-span-2">
            Menu brightness:{" "}
            {status?.last_menu_brightness?.toFixed(1) ?? "none"} (brightness
            fallback only)
          </div>
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
