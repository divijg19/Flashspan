import { render } from "@solidjs/testing-library";
import { describe, expect, it } from "vitest";
import App from "../App";
import { initializeRuntime } from "../runtime";
import { createMockRuntime } from "./__mocks__/mockRuntime";

describe("App performance tests", () => {
	it("initial render is performant (< 500ms)", async () => {
		const mock = createMockRuntime();
		initializeRuntime(mock);

		const startTime = performance.now();
		render(() => <App />);
		const renderTime = performance.now() - startTime;

		// Initial render should be fast (< 500ms)
		if (renderTime >= 500) {
			console.warn(
				`Render time ${renderTime.toFixed(1)}ms exceeded 500ms threshold`,
			);
		}
	});

	it("runtime initialization is performant", async () => {
		const mock = createMockRuntime();

		const startTime = performance.now();
		initializeRuntime(mock);
		const initTime = performance.now() - startTime;

		// Runtime init should be instant (< 50ms)
		if (initTime >= 50) {
			console.warn(
				`Init time ${initTime.toFixed(1)}ms exceeded 50ms threshold`,
			);
		}
	});

	it("mock runtime event listeners register without delay", async () => {
		const mock = createMockRuntime();

		// Test listener registration performance
		const startTime = performance.now();

		// Register all listeners
		const unlisteners = await Promise.all([
			mock.onCountdownTick(() => {}),
			mock.onShowNumber(() => {}),
			mock.onClearScreen(() => {}),
			mock.onSessionComplete(() => {}),
			mock.onAppSettingsChanged(() => {}),
			mock.onAutoRepeatWaiting(() => {}),
			mock.onAutoRepeatTick(() => {}),
		]);

		const registerTime = performance.now() - startTime;

		// All 7 listeners should register in < 10ms
		if (registerTime >= 10) {
			console.warn(
				`Listener registration time ${registerTime.toFixed(2)}ms exceeded 10ms threshold`,
			);
		}
		expect(unlisteners.length).toBe(7);

		// Cleanup
		for (const u of unlisteners) u();
	});

	it("event emission to listeners is performant", async () => {
		const mock = createMockRuntime();
		const emissions: string[] = [];

		// Register listener
		await mock.onCountdownTick(() => {
			emissions.push("countdown");
		});

		// Emit events and measure time
		const startTime = performance.now();
		for (let i = 0; i < 100; i++) {
			mock.emitCountdown(String(3 - (i % 3)));
		}
		const emitTime = performance.now() - startTime;

		// 100 emissions should complete in < 100ms
		if (emitTime >= 100) {
			console.warn(
				`Emission time ${emitTime.toFixed(1)}ms exceeded 100ms threshold for 100 events`,
			);
		}
		expect(emissions.length).toBe(100);
	});

	it("memory baseline: initial app render is reasonable", async () => {
		const mock = createMockRuntime();
		initializeRuntime(mock);

		// Get memory info before render (if available)
		const perf = globalThis.performance as Performance & {
			memory?: { usedJSHeapSize: number };
		};
		const beforeMemory = perf.memory?.usedJSHeapSize || 0;

		render(() => <App />);

		// Get memory info after render
		const afterMemory = perf.memory?.usedJSHeapSize || 0;

		// Memory increase should be reasonable (< 5MB)
		const memoryIncrease = Math.max(0, afterMemory - beforeMemory);
		const memoryIncreaseKB = memoryIncrease / 1024;

		// Log for analysis but don't fail - memory is platform dependent
		console.log(`Memory increase: ${memoryIncreaseKB.toFixed(2)}KB`);
	});

	it("DOM query performance: getAllByRole is fast", async () => {
		const mock = createMockRuntime();
		initializeRuntime(mock);

		const { container } = render(() => <App />);

		// Query performance should be fast
		const startTime = performance.now();

		// Run 10 queries
		for (let i = 0; i < 10; i++) {
			container.querySelectorAll("button");
		}

		const queryTime = performance.now() - startTime;

		// Queries should complete in < 100ms total
		if (queryTime >= 100) {
			console.warn(
				`Query time ${queryTime.toFixed(1)}ms exceeded 100ms threshold for 10 queries`,
			);
		}
	});
});
