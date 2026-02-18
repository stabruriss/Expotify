import { useState, useEffect, useCallback } from "react";
import { checkForUpdate, openUrl, type UpdateInfo } from "../lib/tauri";

const CHECK_INTERVAL_MS = 30 * 60 * 1000; // 30 minutes

export function useUpdateCheck() {
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [dismissed, setDismissed] = useState(false);

  const check = useCallback(async () => {
    try {
      const info = await checkForUpdate();
      if (info.has_update) {
        setUpdateInfo(info);
      }
    } catch {
      // Silently fail -- update check is non-critical
    }
  }, []);

  useEffect(() => {
    const initialTimer = setTimeout(check, 5000);
    const interval = setInterval(check, CHECK_INTERVAL_MS);
    return () => {
      clearTimeout(initialTimer);
      clearInterval(interval);
    };
  }, [check]);

  const openRelease = useCallback(() => {
    if (updateInfo) {
      openUrl(updateInfo.release_url);
    }
  }, [updateInfo]);

  const dismiss = useCallback(() => {
    setDismissed(true);
  }, []);

  return {
    updateAvailable: !!updateInfo?.has_update && !dismissed,
    latestVersion: updateInfo?.latest_version ?? null,
    openRelease,
    dismiss,
  };
}
