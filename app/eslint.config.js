import js from "@eslint/js";
import tseslint from "typescript-eslint";
import react from "eslint-plugin-react";

const HEX_COLOR = "/#[0-9a-fA-F]{3,8}\\b/";
const hexMessage = "Use a color variable from styles.css :root, not a raw hex value.";

export default tseslint.config(
  { ignores: ["dist", "src-tauri"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    plugins: { react },
    settings: { react: { version: "detect" } },
    rules: {
      "react/no-danger": "error",
      "no-restricted-syntax": [
        "error",
        { selector: `Literal[value=${HEX_COLOR}]`, message: hexMessage },
        { selector: `TemplateElement[value.raw=${HEX_COLOR}]`, message: hexMessage },
        { selector: `JSXText[value=${HEX_COLOR}]`, message: hexMessage },
      ],
    },
  },
);
