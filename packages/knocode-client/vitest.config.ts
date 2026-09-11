import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
    // Quiet output: dots + pass/fail summary only; console.log spam suppressed.
    // Verbose view still available: npx vitest run --reporter=verbose --silent=false
    reporters: ["dot"],
    silent: true,
  },
});