import { useCallback, useEffect, useRef, useState } from "react";

import { invoke } from "@tauri-apps/api/core";
import { check, type Update } from "@tauri-apps/plugin-updater";

import packageJson from "../../package.json";
import { Download } from "lucide-react";

import { Button } from "@/components/ui/button";

export type UpdateState = {
  version: string;
  phase: "available" | "downloading" | "ready" | "installing" | "error";
  downloaded: number;
  total: number | null;
};

/**
 * Checks GitHub Releases at launch and every 15 minutes. Signed updates are
 * downloaded in the background when enabled. Installation stays behind an
 * explicit restart action so an update never interrupts work merely because it
 * became available.
 */
export type UpdateCheckResult = "available" | "none" | "unsupported";

export type UpdateController = {
  state: UpdateState | null;
  checking: boolean;
  checkForUpdates: () => Promise<UpdateCheckResult>;
  install: () => Promise<void>;
  retry: () => void;
  fakeUpdateAvailable: () => void;
};

const UPDATE_CHECK_INTERVAL_MS = 15 * 60 * 1000;

export function useUpdate(autoDownloadUpdates: boolean): UpdateController {
  const updateRef = useRef<Update | null>(null);
  const checkRef = useRef<Promise<UpdateCheckResult> | null>(null);
  const stateRef = useRef<UpdateState | null>(null);
  const [state, setState] = useState<UpdateState | null>(null);
  const [checking, setChecking] = useState(false);
  stateRef.current = state;

  const download = useCallback(async (update: Update) => {
    let downloaded = 0;
    let total: number | null = null;
    setState({ version: update.version, phase: "downloading", downloaded, total });

    try {
      await update.download((event) => {
        if (event.event === "Started") {
          total = event.data.contentLength ?? null;
        } else if (event.event === "Progress") {
          downloaded += event.data.chunkLength;
        }
        setState({ version: update.version, phase: "downloading", downloaded, total });
      });
      setState({ version: update.version, phase: "ready", downloaded, total });
    } catch (error) {
      console.error("Failed to download the app update", error);
      setState({ version: update.version, phase: "error", downloaded, total });
    }
  }, []);

  const checkForUpdates = useCallback((): Promise<UpdateCheckResult> => {
    if (import.meta.env.DEV) return Promise.resolve("unsupported");

    const currentState = stateRef.current;
    if (
      currentState?.phase === "available" ||
      currentState?.phase === "downloading" ||
      currentState?.phase === "installing" ||
      currentState?.phase === "ready"
    ) {
      return Promise.resolve("available");
    }
    if (checkRef.current) return checkRef.current;

    setChecking(true);
    const request = (async () => {
      try {
        const update = await check({ timeout: 30_000 });
        if (!update) return "none";
        updateRef.current = update;
        if (autoDownloadUpdates) {
          await download(update);
        } else {
          setState({
            version: update.version,
            phase: "available",
            downloaded: 0,
            total: null,
          });
        }
        return "available";
      } finally {
        setChecking(false);
        checkRef.current = null;
      }
    })();
    checkRef.current = request;
    return request;
  }, [autoDownloadUpdates, download]);

  useEffect(() => {
    if (!autoDownloadUpdates || stateRef.current?.phase !== "available") return;
    const update = updateRef.current;
    if (update) void download(update);
  }, [autoDownloadUpdates, download]);

  useEffect(() => {
    // The development binary has no release bundle to replace and should not
    // contact the production update endpoint on every hot reload.
    if (import.meta.env.DEV) return;

    void checkForUpdates().catch((error) =>
      console.error("Failed to check for app updates", error),
    );
    const interval = window.setInterval(() => {
      void checkForUpdates().catch((error) =>
        console.error("Failed to check for app updates", error),
      );
    }, UPDATE_CHECK_INTERVAL_MS);

    return () => window.clearInterval(interval);
  }, [checkForUpdates]);

  const fakeUpdateAvailable = () => {
    if (!import.meta.env.DEV) return;
    setState({
      version: `${packageJson.version}-dev`,
      phase: "ready",
      downloaded: 0,
      total: null,
    });
  };

  const install = async () => {
    const update = updateRef.current;
    if (import.meta.env.DEV && !update && state?.phase === "ready") {
      setState(null);
      return;
    }
    if (!update || state?.phase !== "ready") return;

    setState({ ...state, phase: "installing" });
    try {
      // Stop managed agent processes before handing control to the installer.
      // Windows exits after launching it; macOS returns after replacing the app
      // and needs the explicit restart that follows.
      await invoke("prepare_for_update");
      await update.install({ restartAfterInstall: true });
      await invoke("restart_after_update");
    } catch (error) {
      console.error("Failed to install the app update", error);
      setState({ ...state, phase: "error" });
    }
  };

  return {
    state,
    install,
    retry: () => {
      if (updateRef.current) void download(updateRef.current);
    },
    checking,
    checkForUpdates,
    fakeUpdateAvailable,
  };
}

export default function UpdateNotice({
  controller,
}: {
  controller: UpdateController;
}) {
  const { state, install, retry } = controller;
  if (!state) return null;

  const label = "Update available";
  const disabled = state.phase === "downloading" || state.phase === "installing";

  return (
    <div className="shrink-0 px-2 py-2">
      <Button
        size="sm"
        variant="ghost"
        className="w-full justify-start px-1.5 text-ui"
        disabled={disabled}
        title={`Lathe ${state.version}: ${label}`}
        aria-label={`Lathe ${state.version}: ${label}`}
        onClick={() => {
          if (state.phase === "available" || state.phase === "error") retry();
          else if (state.phase === "ready") void install();
        }}
      >
        <Download />
        {label}
      </Button>
    </div>
  );
}
