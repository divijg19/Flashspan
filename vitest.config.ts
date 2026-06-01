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
			include: ["src/**"],
			exclude: ["src/wasm/pkg/**", "src/__tests__/**"],
		},
	},
});
