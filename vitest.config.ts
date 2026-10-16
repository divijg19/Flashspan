import solid from "vite-plugin-solid";
import { defineConfig } from "vitest/config";

export default defineConfig({
	plugins: [solid()],
	test: {
		environment: "jsdom",
		globals: true,
		setupFiles: [],
		coverage: {
			provider: "v8",
			enabled: true,
			reporter: ["text", "json"],
			// `src/**` also matches the Rust crate rooted at `src-tauri` via the
			// sibling symlink-free layout, and the v8 provider then hands
			// `src/session.rs` to a JavaScript parser. It logs a RolldownError per
			// file and silently drops them, which reads like a real failure. Rust
			// coverage is reported by cargo-llvm-cov instead.
			include: ["src/**/*.{ts,tsx}"],
			exclude: ["src/wasm/pkg/**", "src/__tests__/**"],
		},
	},
});
