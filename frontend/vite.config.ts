import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  build: {
    lib: {
      entry: "src/OutputPreview.tsx",
      formats: ["es"],
      fileName: "output-preview",
    },
    rollupOptions: {
      external: ["react", "react/jsx-runtime"],
    },
  },
  test: {
    environment: "node",
  },
});
