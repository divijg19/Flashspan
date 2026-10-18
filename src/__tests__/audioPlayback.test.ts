import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { browserRuntime } from "../runtime/browser";

/** An `AudioBufferSourceNode`, kept flat so a test can read what it did. */
interface FakeSource {
	buffer: unknown;
	connect: ReturnType<typeof vi.fn>;
	start: ReturnType<typeof vi.fn>;
	stop: ReturnType<typeof vi.fn>;
}

/**
 * A stand-in for the whole Web Audio graph.
 *
 * The browser is not the thing under test here — the runtime's use of the
 * graph is — so what this records is the shape a test needs to read: which
 * sources were created, whether each was started, and whether the previous one
 * was stopped. Anything richer would only be re-testing Web Audio itself.
 */
class FakeAudioContext {
	/** One graph per page, so this is what a test reads after the first need. */
	static instances: FakeAudioContext[] = [];
	/**
	 * The autoplay policy, in one knob. When it says no, `resume()` changes
	 * nothing and the context never leaves "suspended" — which is exactly what
	 * an engine does outside a direct user gesture.
	 */
	static policyAllowsResume = true;
	/** `createBufferSource()` refuses, which is a play-time device failure. */
	static sourceFails = false;

	state = "suspended";
	currentTime = 0;
	destination = { name: "destination" };
	resume = vi.fn(async () => {
		if (FakeAudioContext.policyAllowsResume) {
			// Genuinely asynchronous, like the real thing: the graph is not
			// running in the same turn that asked for it.
			await Promise.resolve();
			this.state = "running";
		}
	});
	sources: FakeSource[] = [];

	constructor() {
		FakeAudioContext.instances.push(this);
	}

	createGain(): unknown {
		return { gain: { value: 1 }, connect: vi.fn() };
	}

	createBufferSource(): FakeSource {
		if (FakeAudioContext.sourceFails) {
			throw new Error("output device gone");
		}
		const source: FakeSource = {
			buffer: null,
			connect: vi.fn(),
			start: vi.fn(),
			stop: vi.fn(),
		};
		this.sources.push(source);
		return source;
	}

	decodeAudioData(
		_data: ArrayBuffer,
		onSuccess: (buffer: unknown) => void,
		_onError: (error: unknown) => void,
	): void {
		onSuccess({ duration: 0.1 });
	}
}

describe("browser audio playback", () => {
	let fetchMock: ReturnType<typeof vi.fn>;

	/** The one graph the runtime keeps. Throws if it does not exist yet. */
	function graph(): FakeAudioContext {
		const ctx = FakeAudioContext.instances[0];
		if (!ctx) {
			throw new Error("the runtime has not created a graph yet");
		}
		return ctx;
	}

	/**
	 * Mark where this test starts. The graph outlives every test in the file,
	 * and its sources accumulate on it, so a test that indexed from zero would
	 * be reading the sound some earlier test made.
	 */
	function mark(): () => FakeSource[] {
		const from = graph().sources.length;
		return () => graph().sources.slice(from);
	}

	const session = (total_numbers: number) => ({
		digits_per_number: 1,
		number_duration_s: 0.5,
		total_numbers,
		allow_negative_numbers: false,
	});

	beforeEach(() => {
		vi.useFakeTimers();
		vi.setSystemTime(0);
		fetchMock = vi.fn(async () => ({
			ok: true,
			status: 200,
			arrayBuffer: async () => new ArrayBuffer(8),
		}));
		vi.stubGlobal("fetch", fetchMock);
		vi.stubGlobal("AudioContext", FakeAudioContext);
		// jsdom cannot play wav at all, which would put every status read in
		// the unsupported branch and hide the one being asserted.
		vi.spyOn(window.HTMLMediaElement.prototype, "canPlayType").mockReturnValue(
			"probably",
		);
		FakeAudioContext.policyAllowsResume = true;
		FakeAudioContext.sourceFails = false;
	});

	afterEach(async () => {
		await browserRuntime.stopSession();
		FakeAudioContext.policyAllowsResume = true;
		FakeAudioContext.sourceFails = false;
		vi.useRealTimers();
		vi.unstubAllGlobals();
		vi.restoreAllMocks();
	});

	/**
	 * First in the file on purpose: the graph is created by the first thing
	 * that needs it, so a test which has to watch that creation has to run
	 * before anything else needs it.
	 */
	it("brings the graph up from a user gesture, which the policy requires", async () => {
		document.dispatchEvent(new Event("pointerdown"));
		const ctx = graph();

		// `resume()` is async, so the graph is not running at this instant.
		expect(ctx.state).toBe("suspended");
		await vi.advanceTimersByTimeAsync(0);

		expect(ctx.resume).toHaveBeenCalledTimes(1);
		expect(ctx.state).toBe("running");
	});

	/**
	 * Warmup's whole job is to have the bytes decoded before the countdown
	 * ends, and it used to also mark the device ready from a muted play-through
	 * that was rewound straight away — so one early "success" silenced every
	 * later failure, including a device that died mid-session.
	 *
	 * It also runs on the only cold cache a page ever gets, which is where the
	 * two claims meet: the ticks that arrive while the first decode is still in
	 * flight are waiting on the same promise, so when it lands they would both
	 * sound. Only the newest may — a beep that fires a tick late is a beep
	 * attached to the wrong number. That is why this is the first test in the
	 * file to touch the buffers: a load that *succeeds* is cached for the life
	 * of the page, and no later test can be slow again.
	 */
	it("decodes on warmup without claiming anything, and lets only the newest tick sound", async () => {
		const started = mark();
		const pending = new Map<string, () => void>();
		fetchMock.mockImplementation(
			(url: string) =>
				new Promise((resolve) => {
					pending.set(url, () =>
						resolve({
							ok: true,
							status: 200,
							arrayBuffer: async () => new ArrayBuffer(8),
						}),
					);
				}),
		);

		await browserRuntime.setSoundEnabled(true);
		await browserRuntime.startSession(session(2));
		// Past warmup and past two flash ticks, all of them still waiting on the
		// fetch that nothing has released yet.
		await vi.advanceTimersByTimeAsync(3100);
		await vi.advanceTimersByTimeAsync(600);

		expect(pending.size).toBe(3);
		expect(started()).toHaveLength(0);

		// Everything but the beep finishes decoding. Decoding is not playback,
		// so this still proves nothing about the device — and had warmup claimed
		// otherwise, this one early "success" would silence every later failure.
		for (const [url, release] of pending) {
			if (!url.includes("beep")) {
				release();
			}
		}
		await vi.advanceTimersByTimeAsync(0);

		const status = await browserRuntime.getAudioStatus();
		expect(status.detail).toBe("not-started");
		expect(status.available).toBe(true);

		// The beep lands, and of the two ticks that were waiting on it only the
		// newest sounds: a beep a tick late is a beep on the wrong number.
		for (const release of pending.values()) {
			release();
		}
		await vi.advanceTimersByTimeAsync(0);

		expect(started()).toHaveLength(1);
	});

	/**
	 * The browser half of the failure report. A graph that refuses to hand out
	 * a source is a device the page cannot use, and it is the only thing that
	 * can turn into a word the user can read — the warmup deliberately says
	 * nothing either way.
	 *
	 * Driven at play time rather than through a failed decode, because a load
	 * that *succeeds* is cached for the life of the page: the warmup above has
	 * already paid for all three clips, so a decoding failure would only be
	 * reachable before it ran.
	 */
	it("latches a genuine playback failure so the status line can report it", async () => {
		await browserRuntime.setSoundEnabled(true);
		FakeAudioContext.sourceFails = true;

		try {
			await browserRuntime.startSession(session(1));
			await vi.advanceTimersByTimeAsync(3100);

			const status = await browserRuntime.getAudioStatus();
			expect(status.available).toBe(false);
			expect(status.detail).toBe("playback-failed");
		} finally {
			FakeAudioContext.sourceFails = false;
		}
	});

	/**
	 * An engine may suspend the graph again at any time — and it refuses to
	 * resume outside a gesture, which is the case here. Playing into it would
	 * not be inaudible *now*; the source is scheduled against a clock that
	 * never advanced, so it would burst out stale the moment something unlocked
	 * the graph. The tick has to be dropped instead.
	 */
	it("drops a beep rather than scheduling it into a suspended graph", async () => {
		await browserRuntime.setSoundEnabled(true);
		const ctx = graph();
		const started = mark();
		ctx.state = "suspended";
		FakeAudioContext.policyAllowsResume = false;

		try {
			await browserRuntime.startSession(session(1));
			await vi.advanceTimersByTimeAsync(3100);
			expect(started()).toHaveLength(0);
		} finally {
			FakeAudioContext.policyAllowsResume = true;
			ctx.state = "running";
		}
	});

	it("restarts the beep on every flash (cut, not queue)", async () => {
		await browserRuntime.setSoundEnabled(true);
		const started = mark();
		await browserRuntime.startSession(session(2));

		await vi.advanceTimersByTimeAsync(3100);
		expect(started()).toHaveLength(1);
		expect(started()[0].start).toHaveBeenCalledTimes(1);

		await vi.advanceTimersByTimeAsync(600);
		expect(started()).toHaveLength(2);
		// The first beep was stopped rather than left to run: a beep is a flash
		// tick, and a tick that queues behind the last one is a wrong answer.
		expect(started()[0].stop).toHaveBeenCalledTimes(1);
		expect(started()[1].start).toHaveBeenCalledTimes(1);
	});

	it("stops the sounding source when the session stops", async () => {
		await browserRuntime.setSoundEnabled(true);
		const started = mark();
		await browserRuntime.startSession(session(1));
		await vi.advanceTimersByTimeAsync(3100);
		expect(started()).toHaveLength(1);
		expect(started()[0].start).toHaveBeenCalled();

		await browserRuntime.stopSession();
		expect(started()[0].stop).toHaveBeenCalled();
	});

	it("plays nothing while sound is disabled", async () => {
		await browserRuntime.setSoundEnabled(false);
		const started = mark();
		await browserRuntime.startSession(session(1));
		// Past the countdown and the warmup window: still nothing, because a
		// muted session may not make a sound.
		await vi.advanceTimersByTimeAsync(3100);
		expect(started()).toHaveLength(0);

		await browserRuntime.setSoundEnabled(true);
	});

	/**
	 * The masking bug this replaces: once the device had been proven, every
	 * later failure was ignored, so one that died mid-session was reported as
	 * working for the rest of the page. A failure *after* a success is the
	 * whole case, so that is the case this runs in.
	 */
	it("reports a device that dies after it already worked", async () => {
		await browserRuntime.setSoundEnabled(true);
		const started = mark();
		await browserRuntime.startSession(session(2));
		await vi.advanceTimersByTimeAsync(3100);
		expect(started()[0].start).toHaveBeenCalled();
		expect((await browserRuntime.getAudioStatus()).detail).toBe("ready");

		// The device goes away between two flashes.
		FakeAudioContext.sourceFails = true;
		try {
			await vi.advanceTimersByTimeAsync(600);
			const status = await browserRuntime.getAudioStatus();
			expect(status.available).toBe(false);
			expect(status.detail).toBe("playback-failed");
		} finally {
			FakeAudioContext.sourceFails = false;
		}
	});

	it("cuts the sounding source when sound is turned off", async () => {
		await browserRuntime.setSoundEnabled(true);
		const started = mark();
		await browserRuntime.startSession(session(1));
		await vi.advanceTimersByTimeAsync(3100);
		expect(started()[0].start).toHaveBeenCalled();

		// Muting is not only a gate on what comes next: applause outlasts the
		// click that mutes it, so the source has to be stopped too.
		await browserRuntime.setSoundEnabled(false);
		expect(started()[0].stop).toHaveBeenCalled();
	});

	/**
	 * The sound toggle is itself a user gesture, so it is a real chance to
	 * bring a suspended graph back up — and unmuting is exactly when a user has
	 * decided they want to hear something. Without it the next session would
	 * spend its countdown waiting on a graph that could have been woken by the
	 * click that asked for it.
	 */
	it("brings a suspended graph back up when sound is turned on", async () => {
		const ctx = graph();
		await browserRuntime.setSoundEnabled(false);

		// An engine suspends the graph at any time, and refuses to resume
		// outside a gesture — which is what makes the toggle's own gesture
		// worth anything.
		ctx.state = "suspended";
		FakeAudioContext.policyAllowsResume = false;
		const resumesBefore = ctx.resume.mock.calls.length;

		FakeAudioContext.policyAllowsResume = true;
		await browserRuntime.setSoundEnabled(true);
		await vi.advanceTimersByTimeAsync(0);

		expect(ctx.resume.mock.calls.length).toBeGreaterThan(resumesBefore);
		expect(ctx.state).toBe("running");
	});
});
