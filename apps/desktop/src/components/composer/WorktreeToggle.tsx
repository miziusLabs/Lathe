import { Button } from "@/components/ui/button";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useHotkey } from "@/hooks/useHotkey";
import { cn } from "@/lib/utils";

/// Create a separate checkout from the selected project and source branch.
export default function WorktreeToggle({
  on,
  onToggle,
  disabled,
  disabledReason,
}: {
  on: boolean;
  onToggle: () => void;
  disabled?: boolean;
  disabledReason?: string;
}) {
  useHotkey("t", onToggle, { shift: true, enabled: !disabled });

  const toggle = (
    <Button
      type="button"
      variant="ghost"
      size="sm"
      role="switch"
      aria-checked={on}
      disabled={disabled}
      onClick={onToggle}
      className="gap-1.5 px-1.5 text-ui text-muted-foreground aria-checked:text-foreground"
    >
      {/* The track reads as on/off at a glance; the label alone left the
          state ambiguous until you'd read both it and the branch beside it. */}
      <span
        aria-hidden
        className={cn(
          "flex h-3 w-5 shrink-0 items-center rounded-full p-px transition-colors",
          on ? "bg-primary" : "bg-muted-foreground/30",
        )}
      >
        <span
          className={cn(
            "size-2.5 rounded-full bg-background transition-transform",
            on && "translate-x-2",
          )}
        />
      </span>
      Worktree
    </Button>
  );

  // Disabled buttons don't receive pointer events, so a wrapper keeps their
  // explanation hoverable. Enabled buttons remain the keyboard-focusable trigger.
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        {disabled ? (
          <span className="inline-flex cursor-not-allowed">{toggle}</span>
        ) : toggle}
      </TooltipTrigger>
      <TooltipContent side="top" className="h-8 max-w-none whitespace-nowrap">
        {disabledReason ?? "Toggle Worktree"}
        <KbdGroup>
          <Kbd>⌘</Kbd>
          <Kbd>⇧</Kbd>
          <Kbd>T</Kbd>
        </KbdGroup>
      </TooltipContent>
    </Tooltip>
  );
}
