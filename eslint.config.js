// Lint for the frontend. Run with `npm run lint`; CI fails on errors.
//
// The recommended JS and TypeScript rules, plus the Rules of Hooks. Missing
// effect dependencies are warnings, not errors: adding one changes when an
// effect runs, which is a behaviour change to make on purpose, one at a time.
import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";

export default tseslint.config(
  { ignores: ["dist", "src-tauri", "target", "src/bindings", "node_modules", "claude-design", "public"] },
  {
    files: ["src/**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: { globals: globals.browser },
    plugins: { "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      // `_` marks a value kept for its position (a destructured omission, an
      // unused callback parameter).
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_", varsIgnorePattern: "^_" }],
    },
  },
  {
    // Tests build partial fixtures and cast them.
    files: ["src/**/*.test.ts"],
    rules: { "@typescript-eslint/no-explicit-any": "off" },
  },
);
