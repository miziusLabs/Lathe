import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { afterEach, describe, expect, it, vi } from "vitest";

import { APP_MODE, APP_THEME, applyTheme } from "@/lib/theme";
import { CODE_THEME_PAIR } from "@/lib/codeTheme";
import { createSharedCodePlugin } from "@/lib/codePlugin";

const startup = readFileSync(new URL("../../index.html", import.meta.url), "utf8");

afterEach(() => vi.unstubAllGlobals());

describe("fixed One Dark Pro appearance", () => {
  it("starts dark without reading stored preferences or OS appearance", () => {
    expect(startup).toContain(`data-theme="${APP_THEME}"`);
    expect(startup).toContain(`data-mode="${APP_MODE}"`);
    expect(startup).toContain('class="dark"');
    expect(startup).toContain('style="color-scheme: dark"');

    const document = { documentElement: { dataset: {} } };
    const forbidden = () => { throw new Error("Appearance must not read preferences"); };
    const script = startup.match(/<script>([\s\S]*?)<\/script>/)?.[1];
    if (script === undefined) throw new Error("Missing startup script");
    runInNewContext(script, {
      document,
      navigator: { platform: "MacIntel" },
      localStorage: { getItem: forbidden },
      matchMedia: forbidden,
    });
    expect(document.documentElement.dataset).toEqual({ vibrancy: "" });
  });

  it("reasserts dark mode without consulting storage", () => {
    const add = vi.fn();
    const el = { dataset: { theme: "neutral", mode: "light" }, classList: { add }, style: { colorScheme: "light" } };
    vi.stubGlobal("document", { documentElement: el });
    vi.stubGlobal("localStorage", { getItem: () => { throw new Error("Storage unavailable"); } });

    applyTheme();

    expect(el.dataset).toEqual({ theme: "one-dark-pro", mode: "dark" });
    expect(add).toHaveBeenCalledWith("dark");
    expect(el.style.colorScheme).toBe("dark");
  });

  it("uses One Dark Pro in both code renderer slots and the Markdown plugin", () => {
    expect(CODE_THEME_PAIR).toEqual({ light: "one-dark-pro", dark: "one-dark-pro" });
    expect(createSharedCodePlugin(CODE_THEME_PAIR).getThemes()).toEqual([
      "one-dark-pro", "one-dark-pro",
    ]);
  });
});
