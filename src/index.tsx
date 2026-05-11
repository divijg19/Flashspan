/* @refresh reload */
import { render } from "solid-js/web";
import "./index.css";
import App from "./App.tsx";
import { initializeRuntime } from "./runtime";
import { loadWasmCoreBridge } from "./wasm/loader";

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

async function bootstrap(): Promise<void> {
	const isTauri = isTauriEnvironment();
	if (!isTauri) {
		await loadWasmCoreBridge();
	}

	const runtimeImpl = isTauri
		? (await import("./runtime/native")).nativeRuntime
		: (await import("./runtime/browser")).browserRuntime;

	initializeRuntime(runtimeImpl);
	render(() => <App />, root);
}

void bootstrap();
