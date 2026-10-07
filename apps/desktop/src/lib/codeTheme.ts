// Streamdown re-exports Shiki's types; Shiki is a transitive dependency.
import type { BundledTheme } from "streamdown";

export type CodeThemePair = { light: BundledTheme; dark: BundledTheme };

/// Both renderer slots use One Dark Pro so code never follows OS appearance.
export const CODE_THEME_PAIR: CodeThemePair = {
  light: "one-dark-pro",
  dark: "one-dark-pro",
};
