import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Region = {
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
  loading_region: Region;
  menu_region: Region | null;
  dark_threshold: number;
  dark_samples: number;
  bright_samples: number;
  menu_threshold: number;
  menu_samples: number;
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

export const DEFAULT_REGION: Region = {
  x: 1175,
  y: 500,
  width: 200,
  height: 300,
};

export const DEFAULT_MENU_REGION: Region = {
  x: 160,
  y: 200,
  width: 200,
  height: 100,
};

export function findGameWindow(): Promise<GameWindowInfo> {
  return invoke<GameWindowInfo>("find_game_window");
}

export function capturePreview(region: Region): Promise<Preview> {
  return invoke<Preview>("capture_preview", { region });
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
