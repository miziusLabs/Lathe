import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
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

  if (!disabledReason) return toggle;

  // Disabled buttons don't receive pointer events, so the wrapper is the
  // tooltip trigger that keeps the reason hoverable.
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="inline-flex cursor-not-allowed">
          {toggle}
        </span>
      </TooltipTrigger>
      <TooltipContent side="top" className="h-8 max-w-none whitespace-nowrap">
        {disabledReason}
      </TooltipContent>
    </Tooltip>
  );
}
