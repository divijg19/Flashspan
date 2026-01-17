import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Phase = "idle" | "starting" | "countdown" | "flashing" | "complete";

export interface SessionConfig {
  digits_per_number: number;
  number_duration_ms: number;
  delay_between_numbers_ms: number;
  total_numbers: number;
  allow_negative_numbers: boolean;
}

export interface AutoRepeatConfig {
  enabled: boolean;
  repeats: number;
  delay_ms: number;
}

export interface ShowNumber {
  session_id: number;
  index: number;
  total: number;
  value: number;
  running_sum: number;
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

export async function startSession(
  config: SessionConfig,
  autoRepeat?: AutoRepeatConfig | null
): Promise<number> {
  return invoke<number>("start_session", {
    config,
    autoRepeat: autoRepeat ?? null,
  });
}

export async function stopSession(): Promise<void> {
  return invoke<void>("stop_session");
}

export async function cancelAutoRepeat(): Promise<void> {
  return invoke<void>("cancel_auto_repeat");
}

export async function markValidated(session_id: number): Promise<AutoRepeatWaitingPayload | null> {
  return invoke<AutoRepeatWaitingPayload | null>("mark_validated", { sessionId: session_id });
}

export async function acknowledgeComplete(
  session_id: number
): Promise<AutoRepeatWaitingPayload | null> {
  return invoke<AutoRepeatWaitingPayload | null>("acknowledge_complete", { sessionId: session_id });
}

export async function submitAnswer(
  session_id: number,
  provided_sum: number
): Promise<SubmitAnswerResponse> {
  return invoke<SubmitAnswerResponse>("submit_answer", {
    sessionId: session_id,
    providedSum: provided_sum,
  });
}

export async function submitAnswerText(
  session_id: number,
  provided_text: string
): Promise<SubmitAnswerResponse> {
  return invoke<SubmitAnswerResponse>("submit_answer_text", {
    sessionId: session_id,
    providedText: provided_text,
  });
}

// --- Events (typed) ---
export function onCountdownTick(handler: (value: string) => void): Promise<UnlistenFn> {
  return listen<string>("countdown_tick", (event) => handler(String(event.payload ?? "")));
}

export function onShowNumber(handler: (payload: ShowNumber) => void): Promise<UnlistenFn> {
  return listen<ShowNumber>("show_number", (event) => handler(event.payload));
}

export function onClearScreen(handler: () => void): Promise<UnlistenFn> {
  return listen("clear_screen", () => handler());
}

export function onAutoRepeatWaiting(
  handler: (payload: AutoRepeatWaitingPayload) => void
): Promise<UnlistenFn> {
  return listen<AutoRepeatWaitingPayload>("auto_repeat_waiting", (event) =>
    handler(event.payload)
  );
}

export function onSessionComplete(
  handler: (payload: SessionComplete) => void
): Promise<UnlistenFn> {
  return listen<SessionComplete>("session_complete", (event) => handler(event.payload));
}
