/**
 * Browser runtime implementation.
 *
 * This keeps the Solid UI unchanged while swapping the backend execution model
 * to browser timers, localStorage, and Web Audio.
 */

import applauseUrl from "../assets/applause.wav?url";
import beepUrl from "../assets/beep.wav?url";
import buzzerUrl from "../assets/buzzer.wav?url";
import { getWasmCoreBridge } from "../wasm/coreBridge";
import type { Runtime, UnlistenFn } from "./index";
import type {
	AppSettings,
	AutoRepeatConfig,
	AutoRepeatEffective,
	AutoRepeatTickPayload,
	AutoRepeatWaitingPayload,
	ClearScreen,
	ColorScheme,
	SessionComplete,
	SessionConfigEffective,
	SessionConfigInput,
	ShowNumber,
	StartSessionResponse,
	SubmitAnswerResponse,
	ThemeMode,
} from "./types";

type Listener<T> = (payload: T) => void;

interface PendingAutoRepeat {
	sessionId: number;
	remaining: number;
	delayMs: number;
	nextStartAtMs: number;
	config: SessionConfigEffective;
	cancelled: boolean;
	tickId: number | null;
}

interface BrowserSession {
	sessionId: number;
	config: SessionConfigEffective;
	autoRepeat: AutoRepeatEffective | null;
	plannedNumbers: number[] | null;
	plannedSum: number | null;
	numbers: number[];
	sum: number;
	runningSum: number;
	lastPayload: string | null;
	completed: boolean;
	timers: number[];
}

const SETTINGS_KEY = "flashspan.runtime.settings";
const SOUND_KEY = "flashspan.runtime.sound-enabled";
const DEFAULT_SETTINGS: AppSettings = {
	color_scheme: "midnight",
	theme_mode: "dark",
};
const COUNTDOWN_TICKS = [3, 2, 1] as const;
// Flash-timing budget mirror. The authoritative schedule lives in
// src-tauri/src/core/timing.rs; these values must match it exactly.
// Changing either side requires updating the schedule tests on both sides
// (engine.rs timing tests, src/__tests__/flashTiming.test.ts).
// Pre-flash pause only; it must not extend the first number's exposure.
const PRE_FLASH_SETTLE_MS = 100;
// Fixed blank gap between numbers. Not user-configurable.
const INTER_NUMBER_GAP_MS = 100;

const listeners = {
	countdownTick: new Set<Listener<string>>(),
	showNumber: new Set<Listener<ShowNumber>>(),
	clearScreen: new Set<Listener<ClearScreen>>(),
	autoRepeatWaiting: new Set<Listener<AutoRepeatWaitingPayload>>(),
	autoRepeatTick: new Set<Listener<AutoRepeatTickPayload>>(),
	appSettingsChanged: new Set<Listener<AppSettings>>(),
	sessionComplete: new Set<Listener<SessionComplete>>(),
};

let nextSessionId = 1;
let currentSession: BrowserSession | null = null;
let pendingAutoRepeat: PendingAutoRepeat | null = null;
let sessionStarting = false;
let appSettings = loadAppSettings();
let soundEnabled = loadSoundEnabled();
let hasLoggedWasmPlannerReady = false;
let hasLoggedWasmPlannerFallback = false;

const audio = {
	beep: new Audio(beepUrl),
	applause: new Audio(applauseUrl),
	buzzer: new Audio(buzzerUrl),
};

Object.values(audio).forEach((clip) => {
	clip.preload = "auto";
	clip.volume = 0.9;
});

function clamp(value: number, min: number, max: number): number {
	if (Number.isNaN(value)) {
		return min;
	}

	return Math.min(max, Math.max(min, value));
}

function round1(value: number): number {
	return Math.round(value * 10) / 10;
}

function nowMs(): number {
	return Date.now();
}

function toMs(seconds: number): number {
	return Math.max(0, Math.round(seconds * 1000));
}

function safeInt(value: number): number {
	if (!Number.isFinite(value)) {
		return 0;
	}

	return Math.trunc(value);
}

function loadAppSettings(): AppSettings {
	try {
		const raw = window.localStorage.getItem(SETTINGS_KEY);
		if (!raw) {
			return { ...DEFAULT_SETTINGS };
		}

		const parsed = JSON.parse(raw) as Partial<AppSettings>;
		const validColorScheme =
			parsed.color_scheme === "midnight" ||
			parsed.color_scheme === "ivory" ||
			parsed.color_scheme === "crimson" ||
			parsed.color_scheme === "aqua" ||
			parsed.color_scheme === "violet" ||
			parsed.color_scheme === "amber"
				? parsed.color_scheme
				: DEFAULT_SETTINGS.color_scheme;

		return {
			color_scheme: validColorScheme,
			theme_mode: parsed.theme_mode === "light" ? "light" : "dark",
		};
	} catch {
		return { ...DEFAULT_SETTINGS };
	}
}

function persistAppSettings(): void {
	try {
		window.localStorage.setItem(SETTINGS_KEY, JSON.stringify(appSettings));
	} catch {
		// Best-effort only.
	}
}

function loadSoundEnabled(): boolean {
	try {
		const raw = window.localStorage.getItem(SOUND_KEY);
		return raw == null ? true : raw === "true";
	} catch {
		return true;
	}
}

function persistSoundEnabled(): void {
	try {
		window.localStorage.setItem(SOUND_KEY, String(soundEnabled));
	} catch {
		// Best-effort only.
	}
}

function emit<T>(set: Set<Listener<T>>, payload: T): void {
	for (const handler of [...set]) {
		try {
			handler(payload);
		} catch {
			// Listener failures should not break the runtime.
		}
	}
}

function addListener<T>(
	set: Set<Listener<T>>,
	handler: Listener<T>,
): UnlistenFn {
	set.add(handler);
	return () => set.delete(handler);
}

/**
 * Largest integer exactly representable in an f64 (2^53 - 1). The browser
 * accumulates sums in f64, so the worst-case session sum must stay below
 * this for grading to be exact. Must mirror Rust `MAX_EXACT_INTEGER`.
 */
const MAX_EXACT_INTEGER = 2 ** 53 - 1;
const MAX_TOTAL_NUMBERS = 10000;

/**
 * Maximum numbers for a digit width such that even the worst case (every
 * number at maximum magnitude) sums below MAX_EXACT_INTEGER. Must mirror
 * Rust `max_total_for_digits`.
 */
export function maxTotalForDigits(digits: number): number {
	const maxMagnitude = digits <= 1 ? 9 : 10 ** digits - 1;
	return Math.max(
		1,
		Math.min(MAX_TOTAL_NUMBERS, Math.floor(MAX_EXACT_INTEGER / maxMagnitude)),
	);
}

function normalizeSessionConfig(
	input: SessionConfigInput,
): SessionConfigEffective {
	// Policy cap: 15 digits keeps every value exactly representable in f64.
	const digits = clamp(safeInt(input.digits_per_number), 1, 15);
	return {
		digits_per_number: digits,
		number_duration_s: round1(clamp(input.number_duration_s, 0.1, 60)),
		// No inter-number gap field: the gap is fixed (INTER_NUMBER_GAP_MS).
		total_numbers: Math.min(
			clamp(safeInt(input.total_numbers), 1, 10000),
			maxTotalForDigits(digits),
		),
		allow_negative_numbers: Boolean(input.allow_negative_numbers),
	};
}

function normalizeAutoRepeat(
	autoRepeat?: AutoRepeatConfig | null,
): AutoRepeatEffective | null {
	if (!autoRepeat?.enabled) {
		return null;
	}

	// Mirrors the native start_session clamps: at least 1 repeat and a
	// 5s minimum gap so API callers get identical behavior on both runtimes.
	return {
		enabled: true,
		repeats: clamp(safeInt(autoRepeat.repeats), 1, 20),
		delay_s: round1(clamp(autoRepeat.delay_s, 5, 120)),
	};
}

function clearSessionTimers(session: BrowserSession): void {
	for (const timerId of session.timers) {
		window.clearTimeout(timerId);
	}
	session.timers.length = 0;
}

function queueTimer(
	session: BrowserSession,
	delayMs: number,
	callback: () => void,
): void {
	const timerId = window.setTimeout(
		() => {
			session.timers = session.timers.filter((value) => value !== timerId);
			if (
				currentSession?.sessionId !== session.sessionId ||
				session.completed
			) {
				return;
			}

			callback();
		},
		Math.max(0, delayMs),
	);

	session.timers.push(timerId);
}

function emitClearScreen(sessionId: number, index: number | null): void {
	emit(listeners.clearScreen, {
		session_id: sessionId,
		index,
		emitted_at_ms: nowMs(),
	});
}

/**
 * Cut semantics mirroring the native audio worker: restarting a clip cuts
 * whatever it was playing, so rapid flash ticks stay in sync with the
 * visuals instead of queueing behind them.
 */
function playAudio(kind: "beep" | "applause" | "buzzer"): void {
	if (!soundEnabled) {
		return;
	}

	const clip = audio[kind];

	try {
		clip.currentTime = 0;
		// Rapid restarts can reject with AbortError; normalize to a promise
		// so rejections are always observed and swallowed here.
		void Promise.resolve(clip.play()).catch(() => {
			// Best-effort only.
		});
	} catch {
		// Best-effort only (e.g. playback unsupported in this environment).
	}
}

/** Silence all clips; used when a session stops or (re)starts. */
function silenceAudio(): void {
	for (const clip of Object.values(audio)) {
		try {
			clip.pause();
			clip.currentTime = 0;
		} catch {
			// Best-effort only.
		}
	}
}

/**
 * Prime clips while a user gesture is active (session start originates
 * from the Start click): a muted play-through unlocks autoplay and forces
 * decode, so the first flash beep ~3s later plays on time instead of
 * stalling on first use. Inaudible by construction (muted throughout).
 */
function warmupAudio(): void {
	if (!soundEnabled) {
		return;
	}

	for (const clip of Object.values(audio)) {
		try {
			const wasMuted = clip.muted;
			clip.muted = true;
			void Promise.resolve(clip.play())
				.then(() => {
					try {
						clip.pause();
						clip.currentTime = 0;
					} catch {
						// Best-effort only.
					}
				})
				.catch(() => {
					// Best-effort only.
				})
				.finally(() => {
					clip.muted = wasMuted;
				});
		} catch {
			// Best-effort only.
		}
	}
}

function randomInt(maxExclusive: number): number {
	if (maxExclusive <= 1) {
		return 0;
	}

	return Math.floor(Math.random() * maxExclusive);
}

function randomMagnitude(digits: number): number {
	if (digits <= 1) {
		// No leading zero: 1..=9, mirroring the native generator.
		return 1 + randomInt(9);
	}

	const min = 10 ** (digits - 1);
	const span = 10 ** digits - min;
	return min + randomInt(span);
}

export function generateNumber(
	digits: number,
	allowNegative: boolean,
	index: number,
	runningSum: number,
	lastPayload: string | null,
): { payload: string; value: number } {
	let attempt = 0;

	while (attempt < 256) {
		const magnitude = randomMagnitude(digits);
		const negativeAllowed = allowNegative && index > 0;
		const signedValue =
			negativeAllowed && randomInt(2) === 1 ? -magnitude : magnitude;

		if (index === 0 && signedValue < 0) {
			attempt += 1;
			continue;
		}

		if (runningSum + signedValue < 0) {
			attempt += 1;
			continue;
		}

		const payload = String(signedValue);
		if (payload === lastPayload) {
			attempt += 1;
			continue;
		}

		return { payload, value: signedValue };
	}

	return deterministicFallback(lastPayload, digits, runningSum, allowNegative);
}

export function deterministicFallback(
	lastPayload: string | null,
	digits: number,
	runningSum: number,
	allowNegative: boolean,
): { payload: string; value: number } {
	if (lastPayload == null) {
		return { payload: "1", value: 1 };
	}

	const lastVal = Number(lastPayload);
	if (lastVal < 0) {
		const positive = Math.abs(lastVal);
		return { payload: String(positive), value: positive };
	}

	const maxExclusive = digits <= 1 ? 10 : 10 ** digits;
	const next = (lastVal % (maxExclusive - 1)) + 1;

	// Never invent a negative when negatives are disabled (mirrors native).
	if (allowNegative && runningSum - next >= 0) {
		return { payload: String(-next), value: -next };
	}
	return { payload: String(next), value: next };
}

function buildBridgeSeed(): number {
	const cryptoObj = globalThis.crypto;
	if (cryptoObj?.getRandomValues) {
		const buffer = new Uint32Array(1);
		cryptoObj.getRandomValues(buffer);
		return buffer[0];
	}

	return Math.trunc(Date.now() ^ Math.floor(Math.random() * 0x7fffffff));
}

async function resolvePlannedSessionData(
	sessionId: number,
	config: SessionConfigInput,
): Promise<{
	config: SessionConfigEffective;
	numbers: number[];
	sum: number;
} | null> {
	const bridge = getWasmCoreBridge();
	if (!bridge) {
		if (!hasLoggedWasmPlannerFallback) {
			hasLoggedWasmPlannerFallback = true;
			console.info(
				"[runtime/browser] WASM planner unavailable; using JS planner",
			);
		}
		return null;
	}

	if (!hasLoggedWasmPlannerReady) {
		hasLoggedWasmPlannerReady = true;
		console.info("[runtime/browser] WASM planner active");
	}

	let plan: Awaited<ReturnType<typeof bridge.buildSessionPlan>>;
	try {
		plan = await bridge.buildSessionPlan(sessionId, config, buildBridgeSeed());
	} catch {
		if (!hasLoggedWasmPlannerFallback) {
			hasLoggedWasmPlannerFallback = true;
			console.info("[runtime/browser] WASM planner failed; using JS planner");
		}
		return null;
	}

	if (
		!Array.isArray(plan.numbers_generated) ||
		plan.numbers_generated.length === 0
	) {
		return null;
	}

	return {
		config: plan.config_snapshot,
		numbers: plan.numbers_generated.slice(),
		sum: plan.expected_sum,
	};
}

function scheduleAutoRepeatCountdown(): void {
	if (!pendingAutoRepeat || pendingAutoRepeat.cancelled) {
		return;
	}

	const tick = (): void => {
		if (!pendingAutoRepeat || pendingAutoRepeat.cancelled) {
			return;
		}

		const remainingMs = pendingAutoRepeat.nextStartAtMs - nowMs();
		const secondsLeft = Math.max(0, Math.ceil(remainingMs / 1000));

		emit(listeners.autoRepeatTick, {
			session_id: pendingAutoRepeat.sessionId,
			seconds_left: secondsLeft,
			remaining: pendingAutoRepeat.remaining,
		});

		if (remainingMs <= 0) {
			const next = pendingAutoRepeat;
			pendingAutoRepeat = null;
			void startSessionImpl(next.config, {
				enabled: true,
				repeats: next.remaining,
				delay_s: next.delayMs / 1000,
			});
			return;
		}

		pendingAutoRepeat.tickId = window.setTimeout(tick, 1000);
	};

	tick();
}

function armAutoRepeatForSession(
	sessionId: number,
): AutoRepeatWaitingPayload | null {
	const session = currentSession;
	if (
		!session ||
		session.sessionId !== sessionId ||
		!session.completed ||
		!session.autoRepeat
	) {
		return null;
	}

	if (session.autoRepeat.repeats <= 0) {
		return null;
	}

	if (
		pendingAutoRepeat?.sessionId === sessionId &&
		!pendingAutoRepeat.cancelled
	) {
		return {
			session_id: sessionId,
			next_start_at_ms: pendingAutoRepeat.nextStartAtMs,
			remaining: pendingAutoRepeat.remaining,
		};
	}

	const remaining = Math.max(0, session.autoRepeat.repeats - 1);
	session.autoRepeat = {
		...session.autoRepeat,
		repeats: remaining,
	};

	pendingAutoRepeat = {
		sessionId,
		remaining,
		delayMs: toMs(session.autoRepeat.delay_s),
		nextStartAtMs: nowMs() + toMs(session.autoRepeat.delay_s),
		config: session.config,
		cancelled: false,
		tickId: null,
	};

	const payload: AutoRepeatWaitingPayload = {
		session_id: sessionId,
		next_start_at_ms: pendingAutoRepeat.nextStartAtMs,
		remaining: pendingAutoRepeat.remaining,
	};

	emit(listeners.autoRepeatWaiting, payload);
	scheduleAutoRepeatCountdown();
	return payload;
}

function finishSession(session: BrowserSession): void {
	if (session.completed) {
		return;
	}

	session.completed = true;
	clearSessionTimers(session);
	emitClearScreen(session.sessionId, null);
	emit(listeners.sessionComplete, {
		session_id: session.sessionId,
		numbers: session.numbers.slice(),
		sum: session.plannedSum ?? session.sum,
	});
}

async function startSessionImpl(
	config: SessionConfigInput,
	autoRepeat?: AutoRepeatConfig | null,
): Promise<StartSessionResponse> {
	if (sessionStarting) {
		throw new Error("A session is already starting");
	}
	sessionStarting = true;
	try {
		const effectiveAutoRepeat = normalizeAutoRepeat(autoRepeat);
		const plannedSession = await resolvePlannedSessionData(
			nextSessionId,
			config,
		);
		const effectiveConfig =
			plannedSession?.config ?? normalizeSessionConfig(config);

		if (currentSession) {
			clearSessionTimers(currentSession);
		}

		if (pendingAutoRepeat) {
			pendingAutoRepeat.cancelled = true;
			if (pendingAutoRepeat.tickId != null) {
				window.clearTimeout(pendingAutoRepeat.tickId);
			}
			pendingAutoRepeat = null;
		}

		const sessionId = nextSessionId;
		nextSessionId += 1;

		const session: BrowserSession = {
			sessionId,
			config: effectiveConfig,
			autoRepeat: effectiveAutoRepeat,
			plannedNumbers: plannedSession?.numbers ?? null,
			plannedSum: plannedSession?.sum ?? null,
			numbers: [],
			sum: 0,
			runningSum: 0,
			lastPayload: null,
			completed: false,
			timers: [],
		};

		currentSession = session;
		silenceAudio();
		// Prime clips while the Start-click gesture is active so the first
		// flash beep plays on time; the countdown absorbs the unlock cost.
		warmupAudio();
		emitClearScreen(sessionId, null);

		// Event times are pure arithmetic (no RNG): cheap to precompute for
		// any total. Values are generated lazily at fire time from live
		// session state in index order, so sequences are identical to eager
		// generation while startup stays instant for large totals.
		type ScheduledEvent =
			| { at: number; kind: "countdown"; value: string }
			| { at: number; kind: "show"; index: number }
			| { at: number; kind: "clear"; index: number }
			| { at: number; kind: "finish" };

		const numberDurationMs = toMs(effectiveConfig.number_duration_s);
		const events: ScheduledEvent[] = [];
		let at = 0;
		for (const value of COUNTDOWN_TICKS) {
			events.push({ at, kind: "countdown", value: String(value) });
			at += 1000;
		}
		at += PRE_FLASH_SETTLE_MS;
		for (let index = 0; index < effectiveConfig.total_numbers; index += 1) {
			events.push({ at, kind: "show", index });
			at += numberDurationMs;
			events.push({ at, kind: "clear", index });
			at += INTER_NUMBER_GAP_MS;
		}
		events.push({ at, kind: "finish" });

		// Chained single-timer driver: at most one pending timeout, so timer
		// bookkeeping stays O(1) and stopping clears a single handle. Each
		// delay is recomputed against the wall clock, preserving the
		// absolute golden timeline (late events fire ASAP via clamping).
		const startedAtMs = nowMs();
		const fireEvent = (pos: number): void => {
			if (pos >= events.length) {
				return;
			}
			const event = events[pos];
			queueTimer(session, startedAtMs + event.at - nowMs(), () => {
				switch (event.kind) {
					case "countdown": {
						emit(listeners.countdownTick, event.value);
						break;
					}
					case "show": {
						const plannedValue = session.plannedNumbers?.[event.index];
						const generated =
							plannedValue === undefined
								? generateNumber(
										effectiveConfig.digits_per_number,
										effectiveConfig.allow_negative_numbers,
										event.index,
										session.runningSum,
										session.lastPayload,
									)
								: { payload: String(plannedValue), value: plannedValue };
						const { payload, value } = generated;
						const newRunningSum = Math.max(0, session.runningSum + value);
						session.lastPayload = payload;
						session.runningSum = newRunningSum;
						session.numbers.push(value);
						session.sum += value;
						emit(listeners.showNumber, {
							session_id: sessionId,
							index: event.index + 1,
							total: effectiveConfig.total_numbers,
							value,
							running_sum: newRunningSum,
							emitted_at_ms: nowMs(),
						});
						playAudio("beep");
						break;
					}
					case "clear": {
						emitClearScreen(sessionId, event.index + 1);
						break;
					}
					case "finish": {
						finishSession(session);
						break;
					}
				}
				fireEvent(pos + 1);
			});
		};
		fireEvent(0);

		return {
			session_id: sessionId,
			effective_config: effectiveConfig,
			effective_auto_repeat: effectiveAutoRepeat,
		};
	} finally {
		sessionStarting = false;
	}
}

/**
 * Strict answer rule, mirroring Rust `parse_answer_text` in
 * `src-tauri/src/main.rs` exactly: trim ends, strip commas everywhere,
 * optional single sign, ASCII digits only, 64-char cap, i64 range.
 * `BigInt` keeps accept/reject identical to Rust on every input (including
 * beyond f64 precision); the return is exact for every attainable session
 * sum, which the digit-width bound keeps below 2^53.
 */
const INVALID_ANSWER_MESSAGE =
	"Enter a single integer answer (e.g. 42 or -17).";
const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;

function parseProvidedAnswerText(value: string): number {
	const cleaned = value.trim().replace(/,/g, "");
	if (!cleaned || cleaned.length > 64) {
		throw new Error(INVALID_ANSWER_MESSAGE);
	}
	if (!/^[+-]?[0-9]+$/.test(cleaned)) {
		throw new Error(INVALID_ANSWER_MESSAGE);
	}
	const big = BigInt(cleaned);
	if (big < I64_MIN || big > I64_MAX) {
		throw new Error(INVALID_ANSWER_MESSAGE);
	}
	return Number(big);
}

function validateAnswer(
	sessionId: number,
	provided: number,
): SubmitAnswerResponse {
	if (
		!currentSession ||
		currentSession.sessionId !== sessionId ||
		!currentSession.completed
	) {
		throw new Error("No completed session available to validate");
	}

	const expected = currentSession.sum;
	const delta = provided - expected;
	const correct = delta === 0;
	const validation = {
		expected_sum: expected,
		provided_sum: provided,
		correct,
		delta,
	};

	let message: string;
	if (correct) {
		message = `Correct ✅\nExpected answer: ${expected}`;
	} else {
		message = `Incorrect\nExpected answer: ${expected}\nDifference: ${delta > 0 ? "+" : ""}${delta}`;
	}

	return {
		validation,
		auto_repeat_waiting: armAutoRepeatForSession(currentSession.sessionId),
		message,
	};
}

// Testing helpers (internal). Exported to enable deterministic unit tests.
export function __test_clearSessionTimers(session: { timers: number[] }): void {
	clearSessionTimers(session as unknown as BrowserSession);
}

export function __test_setCompletedSession(
	sessionId: number,
	numbers: number[],
) {
	const sum = numbers.reduce((s, n) => s + n, 0);
	currentSession = {
		sessionId,
		config: {
			digits_per_number: 1,
			number_duration_s: 0.1,
			total_numbers: numbers.length,
			allow_negative_numbers: false,
		},
		autoRepeat: null,
		plannedNumbers: numbers,
		plannedSum: sum,
		numbers: numbers.slice(),
		sum,
		runningSum: sum,
		lastPayload: null,
		completed: true,
		timers: [],
	};
}

// Export internal helpers for unit testing
export { parseProvidedAnswerText, validateAnswer };

export const browserRuntime: Runtime = {
	async ping(): Promise<string> {
		return "pong (browser)";
	},

	async getAppSettings(): Promise<AppSettings> {
		return { ...appSettings };
	},

	async setColorScheme(scheme: ColorScheme): Promise<AppSettings> {
		appSettings = { ...appSettings, color_scheme: scheme };
		persistAppSettings();
		emit(listeners.appSettingsChanged, { ...appSettings });
		return { ...appSettings };
	},

	async setThemeMode(mode: ThemeMode): Promise<AppSettings> {
		appSettings = { ...appSettings, theme_mode: mode };
		persistAppSettings();
		emit(listeners.appSettingsChanged, { ...appSettings });
		return { ...appSettings };
	},

	startSession: startSessionImpl,

	async stopSession(): Promise<void> {
		// Cut any lingering flash beeps so audio never outlives the session.
		silenceAudio();
		if (currentSession) {
			clearSessionTimers(currentSession);
			emitClearScreen(currentSession.sessionId, null);
		}

		if (pendingAutoRepeat) {
			pendingAutoRepeat.cancelled = true;
			if (pendingAutoRepeat.tickId != null) {
				window.clearTimeout(pendingAutoRepeat.tickId);
			}
			pendingAutoRepeat = null;
		}

		currentSession = null;
	},

	async cancelAutoRepeat(): Promise<void> {
		if (!pendingAutoRepeat) {
			return;
		}

		pendingAutoRepeat.cancelled = true;
		if (pendingAutoRepeat.tickId != null) {
			window.clearTimeout(pendingAutoRepeat.tickId);
		}
		pendingAutoRepeat = null;
	},

	async markValidated(
		sessionId: number,
	): Promise<AutoRepeatWaitingPayload | null> {
		return armAutoRepeatForSession(sessionId);
	},

	async acknowledgeComplete(
		sessionId: number,
	): Promise<AutoRepeatWaitingPayload | null> {
		return armAutoRepeatForSession(sessionId);
	},

	async submitAnswer(
		sessionId: number,
		providedSum: number,
	): Promise<SubmitAnswerResponse> {
		if (!currentSession || currentSession.sessionId !== sessionId) {
			throw new Error("No matching session available to validate");
		}

		return validateAnswer(sessionId, providedSum);
	},

	async submitAnswerText(
		sessionId: number,
		providedText: string,
	): Promise<SubmitAnswerResponse> {
		return validateAnswer(sessionId, parseProvidedAnswerText(providedText));
	},

	async getSoundEnabled(): Promise<boolean> {
		return soundEnabled;
	},

	async setSoundEnabled(enabled: boolean): Promise<void> {
		soundEnabled = Boolean(enabled);
		persistSoundEnabled();
	},

	async playSound(kind: "beep" | "applause" | "buzzer"): Promise<void> {
		if (!soundEnabled) {
			return;
		}

		const clip = audio[kind];
		try {
			clip.currentTime = 0;
			await Promise.resolve(clip.play()).catch(() => {
				// Best-effort only.
			});
		} catch {
			// Best-effort only.
		}
	},

	async onCountdownTick(handler: (value: string) => void): Promise<UnlistenFn> {
		return addListener(listeners.countdownTick, handler);
	},

	async onShowNumber(
		handler: (payload: ShowNumber) => void,
	): Promise<UnlistenFn> {
		return addListener(listeners.showNumber, handler);
	},

	async onClearScreen(
		handler: (payload: ClearScreen) => void,
	): Promise<UnlistenFn> {
		return addListener(listeners.clearScreen, handler);
	},

	async onAutoRepeatWaiting(
		handler: (payload: AutoRepeatWaitingPayload) => void,
	): Promise<UnlistenFn> {
		return addListener(listeners.autoRepeatWaiting, handler);
	},

	async onAutoRepeatTick(
		handler: (payload: AutoRepeatTickPayload) => void,
	): Promise<UnlistenFn> {
		return addListener(listeners.autoRepeatTick, handler);
	},

	async onAppSettingsChanged(
		handler: (payload: AppSettings) => void,
	): Promise<UnlistenFn> {
		return addListener(listeners.appSettingsChanged, handler);
	},

	async onSessionComplete(
		handler: (payload: SessionComplete) => void,
	): Promise<UnlistenFn> {
		return addListener(listeners.sessionComplete, handler);
	},
};
