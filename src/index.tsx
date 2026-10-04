/* @refresh reload */
import { render } from "solid-js/web";
import "./index.css";
import App from "./App.tsx";
import { initializeRuntime } from "./runtime";
import { getWasmVersion, loadWasmCoreBridge } from "./wasm/loader";

const rootElement = document.getElementById("root");

if (!rootElement) {
	throw new Error("Root element #root not found");
}

const root = rootElement as HTMLElement;

const isTauriEnvironment = (): boolean => {
	const globalWindow = globalThis as typeof globalThis & {
		__TAURI__?: unknown;
		__TAURI_INTERNALS__?: unknown;
	};

	return Boolean(globalWindow.__TAURI__ || globalWindow.__TAURI_INTERNALS__);
};

async function performWasmHealthCheck(): Promise<void> {
	try {
		const version = getWasmVersion();
		if (version) {
			console.info(`[wasm] WASM VERSION: ${version}`);
		}
	} catch (err) {
		const isDev = import.meta.env.DEV;
		console.error("[wasm] Health check failed:", err);
		if (isDev) {
			console.warn(
				"[wasm] WASM health check encountered an error in development mode",
			);
		}
	}
}

async function bootstrap(): Promise<void> {
	const isTauri = isTauriEnvironment();

	// Load the WASM planner in the background rather than ahead of the first
	// paint. Nothing reads the bridge until Start, which is at least a 3.1s
	// countdown away, and the browser runtime already handles it being absent by
	// using the JS planner. Awaiting it here only delayed first paint behind the
	// glue chunk, the wasm download and `instantiate`.
	const wasmReady = isTauri ? null : loadWasmCoreBridge();
	void wasmReady?.then(performWasmHealthCheck, () => {
		// loadWasmCoreBridge already logs and resolves false on failure.
	});

	const runtimeImpl = isTauri
		? (await import("./runtime/native")).nativeRuntime
		: (await import("./runtime/browser")).browserRuntime;

	initializeRuntime(runtimeImpl);
	render(() => <App />, root);
}

void bootstrap();
