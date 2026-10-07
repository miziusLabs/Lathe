import { useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { Pencil } from "lucide-react";

import { Button } from "@/components/ui/button";
import Avatar from "@/components/Avatar";
import type { AccountStatus } from "@/types/events";

export default function ChatGptAccount() {
  const [account, setAccount] = useState<AccountStatus | null>(null);
  const [profilePicture, setProfilePicture] = useState<string | null>(null);
  const [busy, setBusy] = useState<"signin" | "signout" | null>(null);
  const [savingPicture, setSavingPicture] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    const subscription = listen<AccountStatus>("account_changed", ({ payload }) => {
      if (disposed) return;
      setAccount(payload);
      setError(payload.error);
      setBusy(null);
    });

    invoke<AccountStatus>("get_account_status")
      .then((value) => {
        if (!disposed) setAccount(value);
      })
      .catch((reason) => {
        if (!disposed) setError(String(reason));
      });
    invoke<string | null>("get_local_profile_picture")
      .then((path) => {
        if (!disposed) setProfilePicture(path ? convertFileSrc(path) : null);
      })
      .catch((reason) => {
        if (!disposed) setError(String(reason));
      });

    return () => {
      disposed = true;
      void subscription.then((unlisten) => unlisten());
    };
  }, []);

  async function chooseProfilePicture() {
    setError(null);
    try {
      const selected = await open({
        multiple: false,
        filters: [
          {
            name: "Images",
            extensions: [
              "png", "pngs", "apng",
              "jpg", "jpgs", "jpeg", "jpe", "jfif",
              "gif", "webp", "bmp", "ico", "avif", "svg",
            ],
          },
        ],
      });
      if (typeof selected !== "string") return;

      setSavingPicture(true);
      const path = await invoke<string>("save_local_profile_picture", {
        sourcePath: selected,
      });
      setProfilePicture(convertFileSrc(path));
    } catch (reason) {
      setError(`Could not update profile picture: ${String(reason)}`);
    } finally {
      setSavingPicture(false);
    }
  }

  async function signIn() {
    setError(null);
    setBusy("signin");
    try {
      await invoke("sign_in_chatgpt");
    } catch (reason) {
      setError(String(reason));
      setBusy(null);
    }
  }

  async function signOut() {
    setBusy("signout");
    setError(null);
    try {
      await invoke("sign_out_chatgpt");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="flex w-full min-w-0 flex-col gap-2">
      <div className="flex min-w-0 items-center justify-between gap-4">
        <div className="flex min-w-0 items-center gap-3">
          <div className="group relative size-9 shrink-0">
            <Avatar
              key={profilePicture ?? account?.email ?? "signed-out"}
              src={profilePicture}
              name={account?.signedIn ? account.email ?? "ChatGPT" : "ChatGPT"}
              className="size-9 text-ui"
            />
            <button
              type="button"
              aria-label={savingPicture ? "Saving profile picture" : "Change profile picture"}
              title="Change profile picture"
              disabled={savingPicture}
              onClick={() => void chooseProfilePicture()}
              className="absolute inset-0 z-10 flex items-center justify-center rounded-full bg-black/45 text-white opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-wait"
            >
              <Pencil aria-hidden className="size-4" />
            </button>
          </div>
          <div className="min-w-0">
            <p className="text-ui font-medium">ChatGPT account</p>
            <p className="text-ui break-words text-muted-foreground">
              {account?.signedIn
                ? account.email ?? "Connected to ChatGPT"
                : "Connect ChatGPT to use Lathe's built-in coding agent."}
            </p>
          </div>
        </div>
        {busy === "signin" ? (
          <Button
            size="sm"
            className="text-ui"
            variant="outline"
            onClick={() => {
              void invoke("cancel_chatgpt_sign_in")
                .then(() => setBusy(null))
                .catch((reason) => setError(String(reason)));
            }}
          >
            Cancel
          </Button>
        ) : account?.signedIn ? (
          <div className="flex shrink-0 gap-2">
            <Button
              size="sm"
              className="text-ui"
              disabled={busy !== null}
              variant="outline"
              onClick={() => void signIn()}
            >
              Reconnect
            </Button>
            <Button
              size="sm"
              className="text-ui"
              disabled={busy !== null}
              variant="outline"
              onClick={() => void signOut()}
            >
              Sign out
            </Button>
          </div>
        ) : (
          <Button
            size="sm"
            className="text-ui"
            variant="outline"
            disabled={busy !== null}
            onClick={() => void signIn()}
          >
            Continue with ChatGPT
          </Button>
        )}
      </div>
      {busy === "signin" && (
        <p className="text-xs text-muted-foreground">Complete sign-in in your browser.</p>
      )}
      {error && <p role="alert" className="text-xs text-destructive">{error}</p>}
    </div>
  );
}
