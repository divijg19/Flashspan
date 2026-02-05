import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

const fallback = {
  beep: new Audio('/src/assets/beep.wav'),
  applause: new Audio('/src/assets/applause.wav'),
  buzzer: new Audio('/src/assets/buzzer.wav'),
};
Object.values(fallback).forEach(a => { a.preload = 'auto'; a.volume = 0.9; });

export async function playSound(kind: 'beep'|'applause'|'buzzer') {
  try {
    await invoke('play_sound_kind', { kind });
  } catch (e) {
    // native playback failed or not available — fallback to HTML Audio
    const a = fallback[kind];
    try { a.currentTime = 0; await a.play(); } catch { /* ignore */ }
  }
}

export default playSound;

export type Phase = "idle" | "starting" | "countdown" | "flashing" | "complete";

export type ColorScheme =
  | "midnight"
  | "ivory"
  | "crimson"
  | "aqua"
  | "violet"
  | "amber";

export type ThemeMode = "dark" | "light";

export interface AppSettings {
  color_scheme: ColorScheme;
  theme_mode: ThemeMode;
}

export interface SessionConfigInput {
  digits_per_number: number;
  number_duration_s: number;
  delay_between_numbers_s: number;
  total_numbers: number;
  allow_negative_numbers: boolean;
}

export interface SessionConfigEffective {
  digits_per_number: number;
  number_duration_s: number;
  delay_between_numbers_s: number;
  total_numbers: number;
  allow_negative_numbers: boolean;
}

export interface AutoRepeatConfig {
  enabled: boolean;
  repeats: number;
  delay_s: number;
}

export interface AutoRepeatEffective {
  enabled: boolean;
  repeats: number;
  delay_s: number;
}

export interface StartSessionResponse {
  session_id: number;
  effective_config: SessionConfigEffective;
  effective_auto_repeat: AutoRepeatEffective | null;
}

export interface ShowNumber {
  session_id: number;
  index: number;
  total: number;
  value: number;
  running_sum: number;
  emitted_at_ms: number;
}

export interface ClearScreen {
  session_id: number;
  index: number | null;
  emitted_at_ms: number;
}

export interface SessionComplete {
  session_id: number;
  numbers: number[];
  sum: number;
}

export interface AutoRepeatWaitingPayload {
  session_id: number;
  next_start_at_ms: number;
  remaining: number;
}

export interface AutoRepeatTickPayload {
  session_id: number;
  seconds_left: number;
  remaining: number;
}

export interface ValidationResult {
  expected_sum: number;
  provided_sum: number;
  correct: boolean;
  delta: number;
}

export interface SubmitAnswerResponse {
  validation: ValidationResult;
  auto_repeat_waiting: AutoRepeatWaitingPayload | null;
}

export async function ping(): Promise<string> {
  return invoke<string>("ping");
}

export async function getAppSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_app_settings");
}

export async function setColorScheme(color_scheme: ColorScheme): Promise<AppSettings> {
  return invoke<AppSettings>("set_color_scheme", { color_scheme: color_scheme });
}

export async function setThemeMode(theme_mode: ThemeMode): Promise<AppSettings> {
  return invoke<AppSettings>("set_theme_mode", { theme_mode: theme_mode });
}

export async function startSession(
  config: SessionConfigInput,
  autoRepeat?: AutoRepeatConfig | null
): Promise<StartSessionResponse> {
  return invoke<StartSessionResponse>("start_session", {
    config,
    auto_repeat: autoRepeat ?? null,
  });
}

export async function stopSession(): Promise<void> {
  return invoke<void>("stop_session");
}

export async function cancelAutoRepeat(): Promise<void> {
  return invoke<void>("cancel_auto_repeat");
}

export async function markValidated(session_id: number): Promise<AutoRepeatWaitingPayload | null> {
  return invoke<AutoRepeatWaitingPayload | null>("mark_validated", { session_id: session_id });
}

export async function acknowledgeComplete(
  session_id: number
): Promise<AutoRepeatWaitingPayload | null> {
  return invoke<AutoRepeatWaitingPayload | null>("acknowledge_complete", { session_id: session_id });
}

export async function submitAnswer(
  session_id: number,
  provided_sum: number
): Promise<SubmitAnswerResponse> {
  return invoke<SubmitAnswerResponse>("submit_answer", {
    args: {
      session_id: session_id,
      provided_sum: provided_sum,
    },
  });
}

export async function submitAnswerText(
  session_id: number,
  provided_text: string
): Promise<SubmitAnswerResponse> {
  return invoke<SubmitAnswerResponse>("submit_answer_text", {
    args: {
      session_id: session_id,
      provided_text: provided_text,
    },
  });
}

export async function getSoundEnabled(): Promise<boolean> {
  return invoke<boolean>('get_sound_enabled');
}

export async function setSoundEnabled(enabled: boolean): Promise<void> {
  return invoke<void>('set_sound_enabled', { enabled });
}

// --- Events (typed) ---
export function onCountdownTick(handler: (value: string) => void): Promise<UnlistenFn> {
  return listen<string>("countdown_tick", (event) => handler(String(event.payload ?? "")));
}

export function onShowNumber(handler: (payload: ShowNumber) => void): Promise<UnlistenFn> {
  return listen<ShowNumber>("show_number", (event) => handler(event.payload));
}

export function onClearScreen(handler: (payload: ClearScreen) => void): Promise<UnlistenFn> {
  return listen<ClearScreen>("clear_screen", (event) => handler(event.payload));
}

export function onAutoRepeatWaiting(
  handler: (payload: AutoRepeatWaitingPayload) => void
): Promise<UnlistenFn> {
  return listen<AutoRepeatWaitingPayload>("auto_repeat_waiting", (event) =>
    handler(event.payload)
  );
}

export function onAutoRepeatTick(
  handler: (payload: AutoRepeatTickPayload) => void
): Promise<UnlistenFn> {
  return listen<AutoRepeatTickPayload>("auto_repeat_tick", (event) => handler(event.payload));
}

export function onAppSettingsChanged(handler: (payload: AppSettings) => void): Promise<UnlistenFn> {
  return listen<AppSettings>("app_settings_changed", (event) => handler(event.payload));
}

export function onSessionComplete(
  handler: (payload: SessionComplete) => void
): Promise<UnlistenFn> {
  return listen<SessionComplete>("session_complete", (event) => handler(event.payload));
}
