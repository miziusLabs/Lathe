import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import WorktreeToggle from "@/components/composer/WorktreeToggle";
import { TooltipProvider } from "@/components/ui/tooltip";

describe("WorktreeToggle", () => {
  it("is disabled when no Git project is selected", () => {
    const html = renderToStaticMarkup(
      <TooltipProvider>
        <WorktreeToggle
          on={false}
          onToggle={vi.fn()}
          disabled
          disabledReason="Select a Git project to create a Worktree."
        />
      </TooltipProvider>,
    );

    expect(html).toContain('role="switch"');
    expect(html).toContain('aria-checked="false"');
    expect(html).toMatch(/\sdisabled(?:=|>)/);
    expect(html).toContain('data-slot="tooltip-trigger"');
  });

  it("remains interactive when a Git project is selected", () => {
    const html = renderToStaticMarkup(<WorktreeToggle on onToggle={vi.fn()} />);

    expect(html).toContain('role="switch"');
    expect(html).toContain('aria-checked="true"');
    expect(html).not.toMatch(/\sdisabled(?:=|>)/);
    expect(html).not.toContain('data-slot="tooltip-trigger"');
  });
});
