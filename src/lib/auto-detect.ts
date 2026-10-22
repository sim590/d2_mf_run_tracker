import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type NormalizedRegion = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type RegionPercent = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type GameWindowInfo = {
  id: number;
  title: string;
  x: number;
  y: number;
  width: number;
  height: number;
};

export type Preview = {
  image: string;
  brightness: number;
  window: GameWindowInfo;
};

export type AutoDetectConfig = {
  loading_region: NormalizedRegion;
  menu_region: NormalizedRegion | null;
  dark_threshold: number;
  dark_samples: number;
  bright_samples: number;
  menu_threshold: number;
  menu_samples: number;
  menu_dark_samples: number;
  poll_interval_ms: number;
  min_run_seconds: number;
};

export type AutoDetectStatus = {
  running: boolean;
  window: GameWindowInfo | null;
  last_brightness: number | null;
  last_menu_brightness: number | null;
  phase: string;
  last_error: string | null;
  last_event: string | null;
};

export type RunStartPayload = {
  source: string;
};

export type RunEndPayload = {
  duration_ms: number;
  too_short: boolean;
  source: string;
};

export type MenuAnchorResult = {
  text: string;
  font: string;
  score: number;
  ncc_score: number;
  binary_score: number;
  edge_score: number;
  size: number;
  letter_spacing: number;
  x: number;
  y: number;
  width: number;
  height: number;
};

export type MenuTemplateTest = {
  window: GameWindowInfo;
  preview: string;
  results: MenuAnchorResult[];
};

export const DEFAULT_REGION_PERCENT: RegionPercent = {
  x: 45.9,
  y: 34.7,
  width: 7.8,
  height: 20.8,
};

export const DEFAULT_MENU_REGION_PERCENT: RegionPercent = {
  x: 6.25,
  y: 13.89,
  width: 7.81,
  height: 6.94,
};

// A text-only crop of the first left column button at 2560x1440. Adjust the
// region and crop to a single anchor when testing.
export const DEFAULT_MENU_ANCHOR_REGION_PERCENT: RegionPercent = {
  x: 3,
  y: 45.5,
  width: 15,
  height: 4,
};

export const MENU_ANCHOR_OPTIONS = [
  "CINÉMATIQUES",
  "CRÉDITS",
  "QUITTER DIABLO II",
] as const;

export const DEFAULT_MENU_ANCHOR = MENU_ANCHOR_OPTIONS[0];

export function percentToNormalized(region: RegionPercent): NormalizedRegion {
  return {
    x: region.x / 100,
    y: region.y / 100,
    width: region.width / 100,
    height: region.height / 100,
  };
}

export function findGameWindow(): Promise<GameWindowInfo> {
  return invoke<GameWindowInfo>("find_game_window");
}

export function capturePreview(region: NormalizedRegion): Promise<Preview> {
  return invoke<Preview>("capture_preview", { region });
}

export function testMenuTemplates(
  region: NormalizedRegion,
  anchor: string | null,
): Promise<MenuTemplateTest> {
  return invoke<MenuTemplateTest>("test_menu_templates", { region, anchor });
}

export function startAutoDetect(config: AutoDetectConfig): Promise<void> {
  return invoke<void>("auto_detect_start", { config });
}

export function stopAutoDetect(): Promise<void> {
  return invoke<void>("auto_detect_stop");
}

export function getAutoDetectStatus(): Promise<AutoDetectStatus> {
  return invoke<AutoDetectStatus>("auto_detect_status");
}

export function listenRunStart(
  handler: (payload: RunStartPayload) => void,
): Promise<UnlistenFn> {
  return listen<RunStartPayload>("auto-detect://run-start", (event) => {
    handler(event.payload);
  });
}

export function listenRunEnd(
  handler: (payload: RunEndPayload) => void,
): Promise<UnlistenFn> {
  return listen<RunEndPayload>("auto-detect://run-end", (event) => {
    handler(event.payload);
  });
}
