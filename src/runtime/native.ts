/**
 * Native runtime implementation for Tauri desktop.
 *
 * This adapter wraps the existing Tauri API and conforms to the Runtime interface.
 * It enables the UI to stay platform-agnostic while delegating to Tauri for:
 * - IPC command invocation
 * - Event emission and subscription
 * - Audio playback (with fallback to HTML Audio)
 * - Fullscreen and window management
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Runtime, UnlistenFn } from "./index";
import type {
	AppSettings,
	AutoRepeatConfig,
	AutoRepeatTickPayload,
	AutoRepeatWaitingPayload,
	ClearScreen,
	ColorScheme,
	SessionComplete,
	SessionConfigInput,
	ShowNumber,
	StartSessionResponse,
	SubmitAnswerResponse,
	ThemeMode,
} from "./types";

// Audio fallback (in case Tauri audio fails or is not available)
const audioFallback = {
	beep: new Audio("/src/assets/beep.wav"),
	applause: new Audio("/src/assets/applause.wav"),
	buzzer: new Audio("/src/assets/buzzer.wav"),
};
Object.values(audioFallback).forEach((a) => {
	a.preload = "auto";
	a.volume = 0.9;
});

/**
 * Native runtime: wraps existing Tauri API.
 * This is the desktop implementation used by the Tauri app.
 */
export const nativeRuntime: Runtime = {
	// --- Commands ---
	async ping(): Promise<string> {
		return invoke<string>("ping");
	},

	async getAppSettings(): Promise<AppSettings> {
		return invoke<AppSettings>("get_app_settings");
	},

	async setColorScheme(scheme: ColorScheme): Promise<AppSettings> {
		return invoke<AppSettings>("set_color_scheme", { color_scheme: scheme });
	},

	async setThemeMode(mode: ThemeMode): Promise<AppSettings> {
		return invoke<AppSettings>("set_theme_mode", { theme_mode: mode });
	},

	async startSession(
		config: SessionConfigInput,
		autoRepeat?: AutoRepeatConfig | null,
	): Promise<StartSessionResponse> {
		return invoke<StartSessionResponse>("start_session", {
			config,
			auto_repeat: autoRepeat ?? null,
		});
	},

	async stopSession(): Promise<void> {
		return invoke<void>("stop_session");
	},

	async cancelAutoRepeat(): Promise<void> {
		return invoke<void>("cancel_auto_repeat");
	},

	async markValidated(
		sessionId: number,
	): Promise<AutoRepeatWaitingPayload | null> {
		return invoke<AutoRepeatWaitingPayload | null>("mark_validated", {
			session_id: sessionId,
		});
	},

	async acknowledgeComplete(
		sessionId: number,
	): Promise<AutoRepeatWaitingPayload | null> {
		return invoke<AutoRepeatWaitingPayload | null>("acknowledge_complete", {
			session_id: sessionId,
		});
	},

	async submitAnswer(
		sessionId: number,
		providedSum: number,
	): Promise<SubmitAnswerResponse> {
		return invoke<SubmitAnswerResponse>("submit_answer", {
			args: {
				session_id: sessionId,
				provided_sum: providedSum,
			},
		});
	},

	async submitAnswerText(
		sessionId: number,
		providedText: string,
	): Promise<SubmitAnswerResponse> {
		return invoke<SubmitAnswerResponse>("submit_answer_text", {
			args: {
				session_id: sessionId,
				provided_text: providedText,
			},
		});
	},

	async getSoundEnabled(): Promise<boolean> {
		return invoke<boolean>("get_sound_enabled");
	},

	async setSoundEnabled(enabled: boolean): Promise<void> {
		return invoke<void>("set_sound_enabled", { enabled });
	},

	async playSound(kind: "beep" | "applause" | "buzzer"): Promise<void> {
		try {
			await invoke("play_sound_kind", { kind });
		} catch (_e) {
			// Fallback to HTML Audio if Tauri fails
			const audio = audioFallback[kind];
			try {
				audio.currentTime = 0;
				await audio.play();
			} catch {
				/* ignore */
			}
		}
	},

	// --- Event listeners ---
	async onCountdownTick(handler: (value: string) => void): Promise<UnlistenFn> {
		const unlisten = await listen<string>("countdown_tick", (event) => {
			handler(String(event.payload ?? ""));
		});
		return unlisten as UnlistenFn;
	},

	async onShowNumber(
		handler: (payload: ShowNumber) => void,
	): Promise<UnlistenFn> {
		const unlisten = await listen<ShowNumber>("show_number", (event) => {
			handler(event.payload);
		});
		return unlisten as UnlistenFn;
	},

	async onClearScreen(
		handler: (payload: ClearScreen) => void,
	): Promise<UnlistenFn> {
		const unlisten = await listen<ClearScreen>("clear_screen", (event) => {
			handler(event.payload);
		});
		return unlisten as UnlistenFn;
	},

	async onAutoRepeatWaiting(
		handler: (payload: AutoRepeatWaitingPayload) => void,
	): Promise<UnlistenFn> {
		const unlisten = await listen<AutoRepeatWaitingPayload>(
			"auto_repeat_waiting",
			(event) => {
				handler(event.payload);
			},
		);
		return unlisten as UnlistenFn;
	},

	async onAutoRepeatTick(
		handler: (payload: AutoRepeatTickPayload) => void,
	): Promise<UnlistenFn> {
		const unlisten = await listen<AutoRepeatTickPayload>(
			"auto_repeat_tick",
			(event) => {
				handler(event.payload);
			},
		);
		return unlisten as UnlistenFn;
	},

	async onAppSettingsChanged(
		handler: (payload: AppSettings) => void,
	): Promise<UnlistenFn> {
		const unlisten = await listen<AppSettings>(
			"app_settings_changed",
			(event) => {
				handler(event.payload);
			},
		);
		return unlisten as UnlistenFn;
	},

	async onSessionComplete(
		handler: (payload: SessionComplete) => void,
	): Promise<UnlistenFn> {
		const unlisten = await listen<SessionComplete>(
			"session_complete",
			(event) => {
				handler(event.payload);
			},
		);
		return unlisten as UnlistenFn;
	},
};
