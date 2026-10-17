import { render } from "@solidjs/testing-library";
import App from "../App";
import type { StartSessionResponse } from "../runtime";
import { initializeRuntime } from "../runtime";
import { createMockRuntime } from "./__mocks__/mockRuntime";

/**
 * Shared helpers for the browser-runtime tests.
 *
 * Two pieces of boilerplate were repeated across the suite: mounting the app
 * against a mock runtime, and asserting that a `startSession` result is not
 * null. Both encode a contract rather than a scenario, so they belonged in one
 * place — and the null check in particular was copied seven times across four
 * files under two different local variable names.
 */

/**
 * Assert a start was not superseded, and return it non-null.
 *
 * `startSession` returns null only when a stop supersedes the start while its
 * plan is still building. Nothing in these cases does that, so a null here means
 * the runtime broke its own contract rather than declined the session. That is
 * a different and much worse failure than the assertion that follows it, so it
 * gets its own message.
 *
 * Throwing (rather than asserting) keeps the returned value non-null for the
 * caller, so the rest of the test can read as if the start cannot fail.
 */
export function startedOrFail(
	response: StartSessionResponse | null,
): StartSessionResponse {
	if (response === null) {
		throw new Error("startSession unexpectedly returned null");
	}
	return response;
}

/**
 * Mount the app against a fresh mock runtime.
 *
 * Repeated with small variations across eight test files. The variation that
 * matters is *when* the runtime is initialized: the app reads the runtime
 * singleton during mount, so a mock installed after the first render is never
 * seen. Getting that wrong produces a test that passes against a runtime it did
 * not configure.
 *
 * @param configure runs after the mock is built but before the app mounts, so a
 *   test can override a command (`mock.startSession = async () => ...`) and have
 *   the override be the one the app calls.
 */
export async function mountApp(
	configure?: (mock: ReturnType<typeof createMockRuntime>) => void,
) {
	const mock = createMockRuntime();
	configure?.(mock);
	initializeRuntime(mock);

	const rendered = render(() => <App />);
	// Let the app's mount effects settle: it pings the runtime and reads settings
	// before the home screen is interactive.
	await new Promise((resolve) => setTimeout(resolve, 0));

	return { mock, ...rendered };
}
