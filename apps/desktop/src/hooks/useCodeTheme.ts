import { CODE_THEME_PAIR } from "@/lib/codeTheme";
import { useTheme } from "@/hooks/useTheme";

/// All code surfaces share a fixed pair; there is no stored choice or setter.
export function useCodeTheme() {
  return { pair: CODE_THEME_PAIR };
}

export function useCodeThemeWithMode() {
  const { resolvedMode } = useTheme();
  return { pair: CODE_THEME_PAIR, resolvedMode };
}
