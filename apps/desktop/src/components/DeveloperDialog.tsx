import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";

export default function DeveloperDialog({
  open,
  onOpenChange,
  onFakeUpdateAvailable,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  onFakeUpdateAvailable: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-120">
        <DialogHeader>
          <DialogTitle>Developer</DialogTitle>
          <DialogDescription>Development-only tools for testing Lathe.</DialogDescription>
        </DialogHeader>

        <div className="flex items-center justify-between gap-4">
          <div className="flex flex-col gap-1">
            <span className="text-ui font-medium">Updates</span>
            <span className="text-ui text-muted-foreground">
              Show the Sidebar update action without downloading a release.
            </span>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="shrink-0 text-ui"
            onClick={onFakeUpdateAvailable}
          >
            Fake update available
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
