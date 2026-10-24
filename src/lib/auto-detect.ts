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

export type HudTemplateConfig = {
  name: string;
  region: NormalizedRegion;
  template: string;
  threshold: number;
};

export type HudTemplateInput = HudTemplateConfig;

export type HudTemplateResult = {
  name: string;
  score: number;
  ncc_score: number;
  binary_score: number;
  edge_score: number;
  visible: boolean;
  x: number;
  y: number;
};

export type HudTemplateTest = {
  window: GameWindowInfo;
  results: HudTemplateResult[];
  hud_visible: boolean;
};

export type AutoDetectConfig = {
  loading_region: NormalizedRegion;
  menu_region: NormalizedRegion | null;
  menu_templates: MenuTemplateConfig[];
  hud_templates: HudTemplateConfig[];
  dark_threshold: number;
  dark_samples: number;
  bright_samples: number;
  menu_threshold: number;
  menu_samples: number;
  menu_dark_samples: number;
  hud_samples: number;
  loading_samples: number;
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
  last_hud_visible: boolean | null;
  last_hud_scores: HudTemplateResult[];
  last_loading_visible: boolean | null;
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

export type MenuAnchorGroup = "left" | "center" | "right" | "loading";

export const MENU_ANCHOR_OPTIONS = [
  "CINÉMATIQUES",
  "CRÉDITS",
  "QUITTER DIABLO II",
  "JOUER",
  "CRÉER NOUVEAU",
  "CHARGEMENT",
] as const;

export type MenuAnchorText = (typeof MENU_ANCHOR_OPTIONS)[number];

export const MENU_ANCHOR_GROUPS: Record<MenuAnchorText, MenuAnchorGroup> = {
  CINÉMATIQUES: "left",
  CRÉDITS: "left",
  "QUITTER DIABLO II": "left",
  JOUER: "center",
  "CRÉER NOUVEAU": "right",
  CHARGEMENT: "loading",
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
  CHARGEMENT: { x: 44, y: 69, width: 13, height: 5 },
};

export const DEFAULT_MENU_ANCHOR: MenuAnchorText = MENU_ANCHOR_OPTIONS[0];

export type HudMarkerName = "life orb" | "mana orb" | "xp";

export const HUD_MARKERS: readonly HudMarkerName[] = [
  "life orb",
  "mana orb",
  "xp",
] as const;

// The third marker used to be called "skill bar"; keep old saved
// calibrations working by normalizing the name.
const LEGACY_HUD_MARKER_NAMES: Record<string, HudMarkerName> = {
  "skill bar": "xp",
};

export function normalizeHudMarkerName(value: string): HudMarkerName | null {
  const legacy = LEGACY_HUD_MARKER_NAMES[value];
  if (legacy) {
    return legacy;
  }
  return (HUD_MARKERS as readonly string[]).includes(value)
    ? (value as HudMarkerName)
    : null;
}

/// Fixed HUD decision threshold. The live decision uses max(edge, ncc).
export const HUD_THRESHOLD = 0.3;

/// At least two of the three markers must be visible.
export const HUD_REQUIRED_TEMPLATES = 2;

// Starting points for 2560x1440. The template is the whole captured region;
// adjust the region until the static frame fills it, then capture again.
export const DEFAULT_HUD_REGIONS: Record<HudMarkerName, RegionPercent> = {
  "life orb": { x: 19, y: 83, width: 10, height: 17 },
  "mana orb": { x: 71, y: 83, width: 10, height: 17 },
  xp: { x: 30, y: 91, width: 40, height: 2 },
};

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

export function testHudTemplates(
  templates: HudTemplateInput[],
): Promise<HudTemplateTest> {
  return invoke<HudTemplateTest>("test_hud_templates", { templates });
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
