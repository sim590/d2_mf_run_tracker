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

export type MenuTemplateConfig = {
  region: NormalizedRegion;
  text: string;
  group: string;
  font: string;
  size: number;
  letter_spacing: number;
  threshold: number;
};

export type AutoDetectConfig = {
  loading_region: NormalizedRegion;
  menu_region: NormalizedRegion | null;
  menu_templates: MenuTemplateConfig[];
  dark_threshold: number;
  dark_samples: number;
  bright_samples: number;
  menu_threshold: number;
  menu_samples: number;
  menu_dark_samples: number;
  poll_interval_ms: number;
  min_run_seconds: number;
};

export type LiveMenuAnchor = {
  text: string;
  group: string;
  score: number;
  visible: boolean;
};

export type AutoDetectStatus = {
  running: boolean;
  window: GameWindowInfo | null;
  last_brightness: number | null;
  last_menu_brightness: number | null;
  last_menu_score: number | null;
  last_menu_anchors: LiveMenuAnchor[];
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
  group: string;
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

export type MenuAnchorGroup = "left" | "center" | "right";

export const MENU_ANCHOR_OPTIONS = [
  "CINÉMATIQUES",
  "CRÉDITS",
  "QUITTER DIABLO II",
  "JOUER",
  "CRÉER NOUVEAU",
] as const;

export type MenuAnchorText = (typeof MENU_ANCHOR_OPTIONS)[number];

export const MENU_ANCHOR_GROUPS: Record<MenuAnchorText, MenuAnchorGroup> = {
  CINÉMATIQUES: "left",
  CRÉDITS: "left",
  "QUITTER DIABLO II": "left",
  JOUER: "center",
  "CRÉER NOUVEAU": "right",
};

// Starting points for 2560x1440. A text-only crop per anchor; adjust after
// looking at the preview.
export const DEFAULT_MENU_ANCHOR_REGIONS: Record<
  MenuAnchorText,
  RegionPercent
> = {
  CINÉMATIQUES: { x: 3, y: 45.5, width: 15, height: 4 },
  CRÉDITS: { x: 3, y: 51, width: 15, height: 4 },
  "QUITTER DIABLO II": { x: 3, y: 56.5, width: 15, height: 4 },
  JOUER: { x: 43, y: 82, width: 14, height: 4 },
  "CRÉER NOUVEAU": { x: 84, y: 81, width: 14, height: 4 },
};

export const DEFAULT_MENU_ANCHOR: MenuAnchorText = MENU_ANCHOR_OPTIONS[0];

export function defaultMenuAnchorRegion(text: string): RegionPercent {
  return (
    DEFAULT_MENU_ANCHOR_REGIONS[text as MenuAnchorText] ??
    DEFAULT_MENU_ANCHOR_REGIONS[DEFAULT_MENU_ANCHOR]
  );
}

export function menuAnchorGroup(text: string): MenuAnchorGroup | null {
  return MENU_ANCHOR_GROUPS[text as MenuAnchorText] ?? null;
}

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
