/**
 * Browser runtime implementation.
 *
 * This keeps the Solid UI unchanged while swapping the backend execution model
 * to browser timers, localStorage, and Web Audio.
 */

import applauseUrl from "../assets/applause.wav?url";
import beepUrl from "../assets/beep.wav?url";
import buzzerUrl from "../assets/buzzer.wav?url";
import {
	COUNTDOWN_TICK_MS,
	COUNTDOWN_TICKS,
	INTER_NUMBER_GAP_MS,
	PRE_FLASH_SETTLE_MS,
} from "../timing";
import { getWasmCoreBridge, type WasmSessionStep } from "../wasm/coreBridge";
import { formatValidationMessage } from "./grading";
import type { Runtime, UnlistenFn } from "./index";
import {
	generateLocalPlan,
	normalizeAutoRepeat,
	normalizeSessionConfig,
	parseProvidedAnswerText,
} from "./localPlanner";
import type {
	AppSettings,
	AudioStatus,
	AutoRepeatConfig,
	AutoRepeatEffective,
	AutoRepeatTickPayload,
	AutoRepeatWaitingPayload,
	ClearScreen,
	ColorScheme,
	Listener,
	SessionComplete,
	SessionConfigEffective,
	SessionConfigInput,
	ShowNumber,
	SoundKind,
	StartSessionResponse,
	SubmitAnswerResponse,
	ThemeMode,
} from "./types";
import { COLOR_SCHEMES } from "./types";

interface PendingAutoRepeat {
	sessionId: number;
	remaining: number;
	delayMs: number;
	nextStartAtMs: number;
	config: SessionConfigEffective;
	tickId: number | null;
}

interface BrowserSession {
	sessionId: number;
	config: SessionConfigEffective;
	autoRepeat: AutoRepeatEffective | null;
	/**
	 * The full intended plan. Always present after session start: it comes from
	 * the Rust plan on the WASM path, and from eagerly generated values on the
	 * JS fallback path, so grading and the answer reveal never depend on the
	 * partial subset that a throttled tab happened to show.
	 */
	plannedSum: number;
	plannedNumbers: number[];
	completed: boolean;
	/** The auto-repeat countdown has already been armed for this session. */
	autoRepeatArmed: boolean;
	/** At most one pending driver timeout (the driver is a single chain). */
	timerId: number | null;
}

const SETTINGS_KEY = "flashspan.runtime.settings";
const SOUND_KEY = "flashspan.runtime.sound-enabled";
const DEFAULT_SETTINGS: AppSettings = {
	color_scheme: "midnight",
	theme_mode: "dark",
};

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
/**
 * Bumped whenever a session is stopped or superseded. `startSessionImpl`
 * captures it before its await and re-checks after, so a stop that lands during
 * the plan build cannot start a session the user already dismissed. Without it
 * the teardown cleared the *outgoing* session only, and the *incoming* one
 * armed anyway.
 */
let sessionEpoch = 0;
let currentSession: BrowserSession | null = null;
let pendingAutoRepeat: PendingAutoRepeat | null = null;
let sessionStarting = false;
let appSettings = loadAppSettings();
let soundEnabled = loadSoundEnabled();
let hasLoggedWasmPlannerReady = false;
let hasLoggedWasmPlannerFallback = false;
/**
 * What audio has actually done so far: nothing yet, a clip played, or a
 * playback attempt failed. `getAudioStatus` reports this instead of a static
 * capability guess, so it can never claim "ready" for a dead device.
 */
let audioState: "not-started" | "ready" | "failed" = "not-started";

const audio = {
	beep: new Audio(beepUrl),
	applause: new Audio(applauseUrl),
	buzzer: new Audio(buzzerUrl),
};

Object.values(audio).forEach((clip) => {
	// "metadata" rather than "auto": eagerly preloading all three clips fetched
	// ~430KB (applause alone is 350KB) in competition with the wasm download,
	// for audio that is not needed until a session ends. `warmupAudio` decodes
	// the clips under the Start gesture anyway, so the beep still lands on time.
	clip.preload = "metadata";
	clip.volume = 0.9;
});

/**
 * Wall-clock time, used only for the payload fields the UI compares against
 * `Date.now()` (`emitted_at_ms`, `next_start_at_ms`).
 *
 * The driver deliberately does not schedule against this clock: it chains
 * `setTimeout` delays of fixed interval, so a clock step cannot retroactively
 * invalidate pending work.
 */
function nowMs(): number {
	return Date.now();
}

function toMs(seconds: number): number {
	return Math.max(0, Math.round(seconds * 1000));
}

function loadAppSettings(): AppSettings {
	try {
		const raw = window.localStorage.getItem(SETTINGS_KEY);
		if (!raw) {
			return { ...DEFAULT_SETTINGS };
		}

		const parsed = JSON.parse(raw) as Partial<AppSettings>;

		// Validated against the single list in `runtime/types.ts` rather than a
		// second copy of the six schemes: a scheme added to the type but not here
		// would be silently discarded, so a saved preference would reset to the
		// default with no error and no way to tell from the UI.
		const colorScheme: ColorScheme =
			parsed.color_scheme != null &&
			(COLOR_SCHEMES as readonly string[]).includes(parsed.color_scheme)
				? parsed.color_scheme
				: DEFAULT_SETTINGS.color_scheme;

		return {
			color_scheme: colorScheme,
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

/**
 * Apply a settings patch, persist it, and notify listeners.
 *
 * `setColorScheme` and `setThemeMode` were the same function with a different
 * key, down to the copy-and-emit tail, so any change to how settings are saved
 * or announced had to be made twice.
 */
function applyAppSettings(patch: Partial<AppSettings>): AppSettings {
	appSettings = { ...appSettings, ...patch };
	persistAppSettings();
	emit(listeners.appSettingsChanged, { ...appSettings });
	return { ...appSettings };
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
 * Abandon any pending auto-repeat restart.
 *
 * Four call sites need this exact teardown — starting a new session, the
 * hidden-tab abort, stopping, and the explicit cancel control — and each had
 * its own copy of the block. They have to stay in step, which is the same
 * "one algorithm, several copies, no test between them" shape as the planner
 * sign divergence this file used to have.
 *
 * Clearing the handle *is* the cancellation. The block used to set a separate
 * `cancelled` flag first, but it was written in the same breath as the nulling,
 * so every reader's `pendingAutoRepeat?.cancelled` was already false by the
 * time it could be observed — dead state that read as if a timer callback had
 * some other way of surviving. Nulling makes every existing guard see the
 * teardown, so the flag bought nothing.
 *
 * A no-op when nothing is pending, so callers do not need to guard.
 */
function cancelPendingAutoRepeat(): void {
	if (!pendingAutoRepeat) {
		return;
	}
	if (pendingAutoRepeat.tickId != null) {
		window.clearTimeout(pendingAutoRepeat.tickId);
	}
	pendingAutoRepeat = null;
}

/**
 * The part of a session the driver needs to cancel its pending work.
 *
 * `clearSessionTimers` takes this rather than the whole `BrowserSession`, so the
 * type says what the function touches. `BrowserSession` satisfies it
 * structurally, so every caller is unaffected — and the test seam no longer has
 * to cast a bare `{ timerId }` through `unknown` to reach it.
 */
interface TimerHandle {
	timerId: number | null;
}

function clearSessionTimers(session: TimerHandle): void {
	if (session.timerId != null) {
		window.clearTimeout(session.timerId);
		session.timerId = null;
	}
}

/**
 * Schedule the next driver step, the fixed interval after the previous one.
 *
 * One pending handle at a time keeps teardown O(1): stopping a 20,000-event
 * session clears a single timer. The callback also re-checks that this session
 * is still the current one, so a timer armed by a superseded session cannot
 * flash a stale number.
 */
function queueTimer(
	session: BrowserSession,
	delayMs: number,
	callback: () => void,
): void {
	const timerId = window.setTimeout(
		() => {
			session.timerId = null;
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

	session.timerId = timerId;
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
function playAudio(kind: SoundKind): Promise<void> {
	if (!soundEnabled) {
		return Promise.resolve();
	}

	const clip = audio[kind];

	try {
		clip.currentTime = 0;
		// Rapid restarts reject with AbortError, which the runtime itself causes
		// by rewinding for the next beep. Treating that as a device failure would
		// latch "Sound unavailable" for the rest of the page on a single clip, so
		// a rewind-cancelled play must not downgrade a device that has already
		// successfully played.
		//
		// Resolves once playback settles rather than when it finishes; callers on
		// the flash path ignore it, and `playSound` awaits it to match the native
		// runtime, which also resolves once the backend has acknowledged.
		return Promise.resolve(clip.play())
			.then(() => {
				audioState = "ready";
			})
			.catch((error: unknown) => {
				recordAudioFailure(error);
			});
	} catch (error) {
		recordAudioFailure(error);
		return Promise.resolve();
	}
}

/** Records a playback failure unless a working device was already proven. */
function recordAudioFailure(error: unknown): void {
	if (audioState === "ready") {
		return;
	}
	// NotAllowedError is the autoplay policy asking for a gesture, not a missing
	// device: it resolves on the next user-initiated play, so it must not count
	// as a permanent failure either.
	const name =
		error != null && typeof (error as { name?: unknown }).name === "string"
			? (error as { name: string }).name
			: "";
	if (name === "AbortError" || name === "NotAllowedError") {
		return;
	}
	audioState = "failed";
}

/**
 * Stop a clip and rewind it to the start.
 *
 * Two callers need this exact teardown — silencing everything on session stop,
 * and putting a clip back after the muted warm-up play-through — and both wrote
 * it out. `pause()` throws in some engines when the clip has never started
 * playing, so the error is swallowed rather than allowed to escape into a
 * teardown path where there is nothing useful to do with it.
 */
function rewind(clip: HTMLAudioElement): void {
	try {
		clip.pause();
		clip.currentTime = 0;
	} catch {
		// Best-effort only.
	}
}

/** Silence all clips; used when a session stops or (re)starts. */
function silenceAudio(): void {
	for (const clip of Object.values(audio)) {
		rewind(clip);
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
					audioState = "ready";
					rewind(clip);
				})
				.catch(() => {
					if (audioState !== "ready") {
						audioState = "failed";
					}
				})
				.finally(() => {
					clip.muted = wasMuted;
				});
		} catch {
			if (audioState !== "ready") {
				audioState = "failed";
			}
		}
	}
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
	steps: WasmSessionStep[] | null;
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
		// No copy: `plan` was just decoded from wasm and nothing else retains it.
		numbers: plan.numbers_generated,
		sum: plan.expected_sum,
		steps: plan.steps,
	};
}

/**
 * Driver event. A show always carries its value and running sum: the WASM plan
 * supplies them and the local planner generates the whole sequence up front, so
 * there is no fire-time generation left and no null case to handle.
 */
type PlannedDriverEvent =
	| { at: number; kind: "countdown"; value: string }
	| {
			at: number;
			kind: "show";
			index: number;
			value: number;
			runningSum: number;
	  }
	| { at: number; kind: "clear"; index: number }
	| { at: number; kind: "finish" };

/** Serde enum tags the WASM bridge can produce, in schema order. */
const STEP_TAGS = [
	"CountdownTick",
	"ShowNumber",
	"ClearScreen",
	"Complete",
] as const;

/**
 * The tag union every `WasmSessionStep` member is keyed by, distributed over
 * the union so it is the union of each member's key rather than the (empty)
 * intersection `keyof` would give on a union of unrelated objects.
 */
type WasmSessionStepTag = WasmSessionStep extends infer Step
	? Step extends object
		? keyof Step
		: never
	: never;

type _ExpectTrue<T extends true> = T;

/**
 * Compile-time proof that `STEP_TAGS` names every tag the wire union declares.
 *
 * The tags are spelled twice — once in the `WasmSessionStep` type, once in the
 * list above — and a rename that updated only the type would leave
 * `planStepsToEvents` unable to match that tag, silently degrading every
 * browser session to the JS planner. `keyof` alone cannot catch it: a union of
 * single-key objects has an empty key intersection, so this compares the
 * distributed tag union instead. Type-only, so nothing is emitted.
 */
type _AllStepTagsListed = _ExpectTrue<
	WasmSessionStepTag extends (typeof STEP_TAGS)[number] ? true : false
>;

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null;
}

function asNonNegativeInt(value: unknown): number | null {
	if (typeof value !== "number" || !Number.isInteger(value) || value < 0) {
		return null;
	}
	return value;
}

function asFiniteNumber(value: unknown): number | null {
	if (typeof value !== "number" || !Number.isFinite(value)) {
		return null;
	}
	return value;
}

/**
 * Append an event, refusing a timestamp that would collide with the previous
 * one.
 *
 * The driver chains fixed intervals, so two events sharing an `at` would fire in
 * the same turn of the event loop and a flashed number would never be seen.
 * Rust's schedule guarantees a minimum spacing (invariant I2 in `core/timing.rs`),
 * but nothing in TypeScript enforced it, and a zero-delay step from a foreign or
 * corrupt module would otherwise slip through. Rejecting the plan here falls
 * back to the local planner, which cannot produce a collision.
 */
function pushEvent(
	events: PlannedDriverEvent[],
	at: number,
	event: PlannedDriverEvent,
): boolean {
	const last = events[events.length - 1];
	if (last !== undefined && at <= last.at) {
		return false;
	}
	events.push(event);
	return true;
}

/**
 * Convert validated WASM plan steps into driver events. Times accumulate
 * each step's `delay_ms_before_next`, matching engine semantics (the delay
 * follows the step it belongs to). Index-free clears are skipped: the
 * initial one is emitted synchronously at session start and the trailing
 * one by `finishSession`. Returns null on any shape violation so the
 * caller falls back to the local JS planner.
 */
export function planStepsToEvents(
	steps: WasmSessionStep[] | null,
): PlannedDriverEvent[] | null {
	if (!Array.isArray(steps) || steps.length === 0) {
		return null;
	}

	const events: PlannedDriverEvent[] = [];
	let at = 0;
	let shows = 0;

	for (const step of steps) {
		if (!isRecord(step)) {
			return null;
		}
		// serde's externally-tagged enum shape is `{ Tag: payload }`, so exactly
		// one known tag must be present. Walking a hoisted list avoids the
		// `Object.keys(step)` array allocation per step (up to 20,000 of them).
		let tag: (typeof STEP_TAGS)[number] | null = null;
		for (const candidate of STEP_TAGS) {
			if (candidate in step) {
				if (tag != null) {
					return null;
				}
				tag = candidate;
			}
		}
		if (tag == null) {
			return null;
		}
		const body = (step as Record<string, unknown>)[tag];
		if (!isRecord(body)) {
			return null;
		}
		const delay = asNonNegativeInt(body.delay_ms_before_next);
		// Complete carries no delay; every other step must.
		if (tag !== "Complete" && delay == null) {
			return null;
		}

		if (tag === "CountdownTick") {
			if (typeof body.value !== "string") {
				return null;
			}
			if (
				!pushEvent(events, at, { at, kind: "countdown", value: body.value })
			) {
				return null;
			}
		} else if (tag === "ShowNumber") {
			const index = asNonNegativeInt(body.index);
			const value = asFiniteNumber(body.value);
			const runningSum = asFiniteNumber(body.running_sum);
			if (index == null || index < 1 || value == null || runningSum == null) {
				return null;
			}
			// Rust reports 1-based indices; the driver event carries the plan's
			// position, which the emitter adds one back to. Keeping the event
			// 0-based is what makes the WASM and JS-planner paths agree.
			if (
				!pushEvent(events, at, {
					at,
					kind: "show",
					index: index - 1,
					value,
					runningSum,
				})
			) {
				return null;
			}
			shows += 1;
		} else if (tag === "ClearScreen") {
			if (body.index == null) {
				at += delay ?? 0;
				continue;
			}
			const index = asNonNegativeInt(body.index);
			if (index == null || index < 1) {
				return null;
			}
			if (!pushEvent(events, at, { at, kind: "clear", index: index - 1 })) {
				return null;
			}
		}

		at += delay ?? 0;
	}

	if (shows === 0) {
		return null;
	}
	if (!pushEvent(events, at, { at, kind: "finish" })) {
		return null;
	}
	return events;
}

function scheduleAutoRepeatCountdown(): void {
	if (!pendingAutoRepeat) {
		return;
	}

	const tick = (): void => {
		if (!pendingAutoRepeat) {
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
			void startSessionImpl(
				next.config,
				{
					enabled: true,
					repeats: next.remaining,
					delay_s: next.delayMs / 1000,
				},
				true,
			).catch((err: unknown) => {
				// A manual Start that won the race makes this restart throw; observe
				// it so it never surfaces as an unhandled rejection.
				console.info("[runtime/browser] auto-repeat restart skipped:", err);
			});
			return;
		}

		pendingAutoRepeat.tickId = window.setTimeout(tick, 1000);
	};

	tick();
}

/**
 * The payload for a pending restart, from the `pendingAutoRepeat` it belongs to.
 *
 * Built in two places inside `armAutoRepeatForSession` — the cached path that
 * reports an already-armed countdown, and the fresh path that has just armed one
 * — and the two literals had to agree field for field for the UI to treat them
 * the same.
 */
function waitingPayload(
	sessionId: number,
	pending: NonNullable<typeof pendingAutoRepeat>,
): AutoRepeatWaitingPayload {
	return {
		session_id: sessionId,
		next_start_at_ms: pending.nextStartAtMs,
		remaining: pending.remaining,
	};
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

	// Already armed for this session: report the pending countdown rather than
	// re-arming it. Optional chaining makes this false when nothing is pending,
	// and anything non-null is by definition not cancelled.
	if (pendingAutoRepeat?.sessionId === sessionId) {
		return waitingPayload(sessionId, pendingAutoRepeat);
	}

	// `repeats` counts the sessions still to run. Decrementing to zero still means
	// "schedule exactly one more", matching native, where the guard
	// `remaining > 0` lives in the completion path (`run_session_plan` only arms
	// `awaiting_validation_session_id` when repeats remain) rather than here.
	// The chain stops because `normalizeAutoRepeat` treats a zero count as "no
	// auto-repeat" instead of clamping it back up to 1, which is what made it
	// loop forever.
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
		tickId: null,
	};

	const payload = waitingPayload(sessionId, pendingAutoRepeat);

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
	// The reveal reflects the session's plan, matching native: the full
	// sequence and its total, not a subset of what was actually painted.
	emit(listeners.sessionComplete, {
		session_id: session.sessionId,
		numbers: session.plannedNumbers.slice(),
		sum: session.plannedSum,
	});
}

async function startSessionImpl(
	config: SessionConfigInput,
	autoRepeat?: AutoRepeatConfig | null,
	// Set by the auto-repeat restart, which carries a plan that is already
	// normalized and whose `repeats` may legitimately be 0 ("nothing left").
	// Re-applying the user-facing clamp turned that 0 back into 1 and restarted
	// the chain forever. Native never re-normalizes: it reuses the stored
	// `AutoRepeatPlan` with its decremented `remaining`.
	alreadyNormalized = false,
): Promise<StartSessionResponse | null> {
	if (sessionStarting) {
		throw new Error("A session is already starting");
	}
	sessionStarting = true;
	try {
		const effectiveAutoRepeat = alreadyNormalized
			? (autoRepeat ?? null)
			: normalizeAutoRepeat(autoRepeat);

		// Tear the outgoing session down *before* awaiting the plan. The plan
		// build (and its WASM deserialize) can take long enough for the previous
		// session's chained timer to fire again mid-teardown and flash a stale
		// number; clearing first and dropping `currentSession` makes the timer
		// guard fail closed.
		if (currentSession) {
			clearSessionTimers(currentSession);
			currentSession = null;
		}

		cancelPendingAutoRepeat();

		const epochAtStart = sessionEpoch;
		const plannedSession = await resolvePlannedSessionData(
			nextSessionId,
			config,
		);
		if (epochAtStart !== sessionEpoch) {
			// Stopped (or superseded) while the plan was building. Baling out here
			// is what keeps Escape during the "starting" phase from landing a full
			// session anyway.
			return null;
		}
		const effectiveConfig =
			plannedSession?.config ?? normalizeSessionConfig(config);

		// Defence against a plan that disagrees with the config it was built from.
		// Rust always produces exactly `total_numbers` values, so this is only
		// reachable through a lying or corrupt bridge — but it must fail the
		// session rather than flash a different count than the payload says.
		if (
			plannedSession != null &&
			plannedSession.numbers.length !== effectiveConfig.total_numbers
		) {
			throw new Error("planned session disagrees with its config");
		}

		const sessionId = nextSessionId;
		nextSessionId += 1;

		// Decided once, here, so grading and the reveal read a value that cannot
		// depend on how much of the session actually painted. When there is no
		// Rust plan this generates the sequence a single time; the flash events
		// below reuse these same numbers rather than generating again.
		const { numbers: plannedNumbers, sum: plannedSum } =
			plannedSession ?? generateLocalPlan(effectiveConfig);
		const session: BrowserSession = {
			sessionId,
			config: effectiveConfig,
			autoRepeat: effectiveAutoRepeat,
			plannedSum,
			plannedNumbers,
			completed: false,
			autoRepeatArmed: false,
			timerId: null,
		};

		currentSession = session;
		silenceAudio();
		// Prime clips while the Start-click gesture is active so the first
		// flash beep plays on time; the countdown absorbs the unlock cost.
		warmupAudio();
		emitClearScreen(sessionId, null);

		// Single-source execution when the WASM planner yields valid steps:
		// times, values, and running sums come straight from the Rust plan.
		// Otherwise the local JS planner builds the same schedule shape around
		// `plannedNumbers` — the one sequence generated above, not a fresh draw —
		// so the events, the stored answer key, and the reveal cannot disagree.
		const numberDurationMs = toMs(effectiveConfig.number_duration_s);
		let events: PlannedDriverEvent[] | null = plannedSession?.steps
			? planStepsToEvents(plannedSession.steps)
			: null;
		if (events == null) {
			const local: PlannedDriverEvent[] = [];
			// `pushEvent` refuses colliding timestamps, so a plan that would fire
			// two stimuli in one turn of the event loop fails the whole session
			// rather than silently showing an unseeable number.
			const push = (event: PlannedDriverEvent): void => {
				if (!pushEvent(local, event.at, event)) {
					clearSessionTimers(session);
					currentSession = null;
					throw new Error("session plan would overlap two events");
				}
			};

			let at = 0;
			for (const value of COUNTDOWN_TICKS) {
				push({ at, kind: "countdown", value: String(value) });
				at += COUNTDOWN_TICK_MS;
			}
			at += PRE_FLASH_SETTLE_MS;

			let running = 0;
			for (let index = 0; index < plannedNumbers.length; index += 1) {
				const value = plannedNumbers[index];
				running += value;
				push({ at, kind: "show", index, value, runningSum: running });
				at += numberDurationMs;
				push({ at, kind: "clear", index });
				at += INTER_NUMBER_GAP_MS;
			}
			push({ at, kind: "finish" });
			events = local;
		}

		// Chained single-timer driver: at most one pending timeout, so timer
		// bookkeeping stays O(1) and stopping clears a single handle.
		//
		// Each event is scheduled the fixed interval after the previous one,
		// *not* against an absolute grid. Anchoring to absolute times makes the
		// previous event's delivery lateness eat into this one's window: the
		// countdown's paint tail and the countdown->flashing view transition
		// delay the first show, which then shortened the first number's exposure
		// by the same amount, so it visibly flashed for less than the rest.
		// Fixed intervals make exposure exact by construction: a late callback
		// can only delay when an event lands, never shorten how long the
		// previous one stays on screen.
		const previousAtMs = (pos: number): number =>
			pos === 0 ? 0 : events[pos - 1].at;
		const fireEvent = (pos: number): void => {
			if (pos >= events.length) {
				return;
			}
			const event = events[pos];
			queueTimer(session, event.at - previousAtMs(pos), () => {
				switch (event.kind) {
					case "countdown": {
						emit(listeners.countdownTick, event.value);
						break;
					}
					case "show": {
						// Values come straight from the plan, whether that is the
						// Rust plan or the local planner's eager sequence.
						const value = event.value;
						emit(listeners.showNumber, {
							session_id: sessionId,
							index: event.index + 1,
							total: effectiveConfig.total_numbers,
							value,
							running_sum: event.runningSum,
							emitted_at_ms: nowMs(),
						});
						void playAudio("beep");
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
				// Always the next event, in order and in full. With fixed
				// intervals a backlog cannot strobe: the next event is always at
				// least one spacing after *this* delivery, so events that were
				// merely late fire once each rather than back to back. A
				// backgrounded tab therefore stretches the session rather than
				// dropping stimulus the user never saw.
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

export function validateAnswer(
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

	// Grade the intended plan's total, matching native. A late delivery delays
	// the whole session rather than shortening it, so the target is always the
	// complete plan.
	const expected = currentSession.plannedSum;
	const delta = provided - expected;
	const correct = delta === 0;
	const validation = {
		expected_sum: expected,
		provided_sum: provided,
		correct,
		delta,
	};

	if (correct) {
		void playAudio("applause");
	} else {
		void playAudio("buzzer");
	}
	const message = formatValidationMessage(correct, expected, delta);

	return {
		validation,
		auto_repeat_waiting: armAutoRepeatForSession(currentSession.sessionId),
		message,
	};
}

/**
 * A session abandoned while the tab is hidden must not run on forever.
 *
 * The driver chains fixed intervals, so a late delivery stretches the session
 * rather than dropping stimulus. Chrome throttles a deeply-nested chained timer
 * to one wake per minute once a page has been hidden past five minutes, which
 * would leave a 10,000-number session crawling for hours with nothing to show
 * for it. Interrupting the session at the boundary keeps that case finite and
 * tells the user what happened, instead of a screen that appears stuck.
 *
 * The threshold is a product decision, not a correctness one: the schedule
 * itself never needed it.
 */
const MAX_HIDDEN_SESSION_MS = 30_000;
let hiddenAtMs: number | null = null;
let hiddenAbortArmed = false;
let interruptionListener: (() => void) | null = null;

function armHiddenAbort(listener: () => void): void {
	// Always take the newest handler, whether or not the DOM listener is already
	// installed: the first caller registers it, later ones just swap who is
	// notified.
	interruptionListener = listener;
	if (hiddenAbortArmed || typeof document === "undefined") {
		return;
	}
	hiddenAbortArmed = true;
	document.addEventListener("visibilitychange", () => {
		if (document.visibilityState === "hidden") {
			hiddenAtMs = Date.now();
			return;
		}
		const hiddenSince = hiddenAtMs;
		hiddenAtMs = null;
		if (hiddenSince == null || !currentSession || currentSession.completed) {
			return;
		}
		if (Date.now() - hiddenSince >= MAX_HIDDEN_SESSION_MS) {
			clearSessionTimers(currentSession);
			cancelPendingAutoRepeat();
			currentSession = null;
			sessionEpoch += 1;
			interruptionListener?.();
		}
	});
}

export const browserRuntime: Runtime = {
	async ping(): Promise<string> {
		return "pong (browser)";
	},

	async onSessionInterrupted(handler: () => void): Promise<UnlistenFn> {
		armHiddenAbort(handler);
		// Keep a stable unlisten for the lifetime of the page; the listener is
		// registered once and reads the handler through the closure.
		return () => {};
	},

	async getAppSettings(): Promise<AppSettings> {
		return { ...appSettings };
	},

	async setColorScheme(scheme: ColorScheme): Promise<AppSettings> {
		return applyAppSettings({ color_scheme: scheme });
	},

	async setThemeMode(mode: ThemeMode): Promise<AppSettings> {
		return applyAppSettings({ theme_mode: mode });
	},

	startSession: startSessionImpl,

	async stopSession(): Promise<void> {
		// Cut any lingering flash beeps so audio never outlives the session.
		silenceAudio();
		if (currentSession) {
			clearSessionTimers(currentSession);
			emitClearScreen(currentSession.sessionId, null);
		}

		cancelPendingAutoRepeat();

		currentSession = null;
		sessionEpoch += 1;
	},

	async cancelAutoRepeat(): Promise<void> {
		cancelPendingAutoRepeat();
	},

	async acknowledgeComplete(
		sessionId: number,
	): Promise<AutoRepeatWaitingPayload | null> {
		const session = currentSession;
		if (!session || session.sessionId !== sessionId || !session.completed) {
			return null;
		}
		// The auto-repeat countdown is armed once per completed session; a
		// duplicate acknowledge reports null, matching native, instead of
		// replaying the cached payload.
		if (session.autoRepeatArmed) {
			return null;
		}
		session.autoRepeatArmed = true;
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

	async getAudioStatus(): Promise<AudioStatus> {
		const supported =
			typeof window.Audio === "function" &&
			audio.beep.canPlayType("audio/wav") !== "";
		const available = supported && audioState !== "failed";
		const detail = !supported
			? "wav-playback-unsupported"
			: audioState === "ready"
				? "ready"
				: audioState === "failed"
					? "playback-failed"
					: "not-started";
		return {
			enabled: soundEnabled,
			available,
			detail,
		};
	},

	async playSound(kind: SoundKind): Promise<void> {
		// `playAudio` already runs the full state machine — gate, rewind,
		// success/failure classification — so a UI-initiated probe and a flash
		// beep cannot disagree about whether the device works. This is a thin
		// awaitable wrapper rather than a second copy of that logic.
		return playAudio(kind);
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

// ---------------------------------------------------------------------------
// Test seams
//
// Internals exported purely so tests can drive them without a full session.
// They are not part of the app's API: nothing in `src/` outside this module and
// the test suite reads them.
//
//   `planStepsToEvents` — the WASM-plan-to-driver-event converter, a pure
//   function with no ambient state, declared `export` where it is defined.
//
//   `validateAnswer` — the answer-grading entry point, exercised directly
//   because it is where the answer key comes from.
//
//   `__test_clearSessionTimers` / `__test_setCompletedSession` — thin wrappers
//   around module-private state. Named with the `__test_` prefix so it is
//   obvious at the call site that this is a seam, not API.
//
// The pure planner that mirrors the Rust core — `normalizeSessionConfig`,
// `normalizeAutoRepeat`, `maxTotalForDigits`, `generateNumber`,
// `deterministicFallback`, `generateLocalPlan`, `parseProvidedAnswerText` and
// `parseWithNumberFallback` — now lives in `./localPlanner`, which is where the
// wasm-parity suites reach it.
// ---------------------------------------------------------------------------

/** Simulate a driver timer firing, so `clearSessionTimers` can be unit-tested. */
export function __test_clearSessionTimers(session: {
	timerId: number | null;
}): void {
	clearSessionTimers(session);
}

/**
 * Install a completed session, so answer grading can be tested without running
 * a whole session to completion.
 */
export function __test_setCompletedSession(
	sessionId: number,
	numbers: number[],
): void {
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
		plannedSum: sum,
		plannedNumbers: numbers.slice(),
		completed: true,
		autoRepeatArmed: false,
		timerId: null,
	};
}
