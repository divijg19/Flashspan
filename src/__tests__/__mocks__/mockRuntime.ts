import type {
	AppSettings,
	AutoRepeatTickPayload,
	AutoRepeatWaitingPayload,
	ClearScreen,
	Runtime,
	SessionComplete,
	ShowNumber,
} from "../../runtime";

type Listener<T> = (payload: T) => void;

export function createMockRuntime() {
	const listeners = {
		countdownTick: new Set<Listener<string>>(),
		showNumber: new Set<Listener<ShowNumber>>(),
		clearScreen: new Set<Listener<ClearScreen>>(),
		autoRepeatWaiting: new Set<Listener<AutoRepeatWaitingPayload>>(),
		autoRepeatTick: new Set<Listener<AutoRepeatTickPayload>>(),
		appSettingsChanged: new Set<Listener<AppSettings>>(),
		sessionComplete: new Set<Listener<SessionComplete>>(),
	};

	return {
		// commands
		async ping() {
			return "pong (mock)";
		},
		async getAppSettings() {
			return { color_scheme: "midnight", theme_mode: "dark" };
		},
		async setColorScheme(s) {
			return { color_scheme: s, theme_mode: "dark" };
		},
		async setThemeMode(m) {
			return { color_scheme: "midnight", theme_mode: m };
		},
		async startSession() {
			return {
				session_id: 1,
				effective_config: {
					digits_per_number: 1,
					number_duration_s: 0.5,
					delay_between_numbers_s: 0,
					total_numbers: 1,
					allow_negative_numbers: false,
				},
				effective_auto_repeat: null,
			};
		},
		async stopSession() {},
		async cancelAutoRepeat() {},
		async markValidated() {
			return null;
		},
		async acknowledgeComplete() {
			return null;
		},
		async submitAnswer(_sessionId: number, providedSum: number) {
			return {
				validation: {
					expected_sum: providedSum,
					provided_sum: providedSum,
					correct: true,
					delta: 0,
				},
				auto_repeat_waiting: null,
			};
		},
		async submitAnswerText(_sessionId: number, providedText: string) {
			const v = Number(providedText.replace(/[\s,]+/g, "")) || 0;
			return {
				validation: {
					expected_sum: v,
					provided_sum: v,
					correct: true,
					delta: 0,
				},
				auto_repeat_waiting: null,
			};
		},
		async getSoundEnabled() {
			return true;
		},
		async setSoundEnabled() {},
		async playSound() {},

		// event listeners
		async onCountdownTick(handler: (v: string) => void) {
			listeners.countdownTick.add(handler);
			return () => listeners.countdownTick.delete(handler);
		},
		async onShowNumber(handler: (p: ShowNumber) => void) {
			listeners.showNumber.add(handler);
			return () => listeners.showNumber.delete(handler);
		},
		async onClearScreen(handler: (p: ClearScreen) => void) {
			listeners.clearScreen.add(handler);
			return () => listeners.clearScreen.delete(handler);
		},
		async onAutoRepeatWaiting(handler: (p: AutoRepeatWaitingPayload) => void) {
			listeners.autoRepeatWaiting.add(handler);
			return () => listeners.autoRepeatWaiting.delete(handler);
		},
		async onAutoRepeatTick(handler: (p: AutoRepeatTickPayload) => void) {
			listeners.autoRepeatTick.add(handler);
			return () => listeners.autoRepeatTick.delete(handler);
		},
		async onAppSettingsChanged(handler: (p: AppSettings) => void) {
			listeners.appSettingsChanged.add(handler);
			return () => listeners.appSettingsChanged.delete(handler);
		},
		async onSessionComplete(handler: (p: SessionComplete) => void) {
			listeners.sessionComplete.add(handler);
			return () => listeners.sessionComplete.delete(handler);
		},

		// helpers to emit events in tests
		emitCountdown(v: string) {
			for (const h of listeners.countdownTick) h(v);
		},
		emitShowNumber(payload: ShowNumber) {
			for (const h of listeners.showNumber) h(payload);
		},
		emitClearScreen(payload: ClearScreen) {
			for (const h of listeners.clearScreen) h(payload);
		},
		emitSessionComplete(payload: SessionComplete) {
			for (const h of listeners.sessionComplete) h(payload);
		},
	} as Runtime & {
		emitCountdown: (v: string) => void;
		emitShowNumber: (p: ShowNumber) => void;
		emitClearScreen: (p: ClearScreen) => void;
		emitSessionComplete: (p: SessionComplete) => void;
	};
}
