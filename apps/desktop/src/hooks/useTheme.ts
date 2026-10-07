import { useEffect } from "react";
import { APP_THEME, APP_MODE, applyTheme } from "@/lib/theme";

/// Reassert the fixed appearance stamped before first paint. Stored preferences
/// and OS appearance never override One Dark Pro.
export function useTheme(): {
  theme: typeof APP_THEME;
  resolvedMode: typeof APP_MODE;
} {
  useEffect(applyTheme, []);
  return { theme: APP_THEME, resolvedMode: APP_MODE };
}
