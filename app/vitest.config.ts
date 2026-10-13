import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
    // The contrast test reads the color tokens from the stylesheet.
    css: { include: [/styles\.css/] },
  },
});
