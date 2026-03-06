import { useState, useEffect, useCallback } from "react";
import { spotifyGetDevices, spotifyTransferPlayback } from "../lib/tauri";
import type { SpotifyDevice } from "../types";

interface DevicePickerProps {
  onClose: () => void;
}

function deviceIcon(type: string): string {
  switch (type.toLowerCase()) {
    case "computer": return "\uD83D\uDCBB";
    case "smartphone": return "\uD83D\uDCF1";
    case "speaker": return "\uD83D\uDD0A";
    case "tv": return "\uD83D\uDCFA";
    case "cast_video":
    case "castaudio": return "\uD83D\uDCE1";
    default: return "\uD83C\uDFB5";
  }
}

export function DevicePicker({ onClose: _onClose }: DevicePickerProps) {
  const [devices, setDevices] = useState<SpotifyDevice[]>([]);
  const [loading, setLoading] = useState(true);
  const [transferring, setTransferring] = useState<string | null>(null);

  const fetchDevices = useCallback(async () => {
    setLoading(true);
    try {
      const devs = await spotifyGetDevices();
      setDevices(devs);
    } catch (e) {
      console.error("Failed to fetch devices:", e);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchDevices();
  }, [fetchDevices]);

  const handleTransfer = async (deviceId: string) => {
    setTransferring(deviceId);
    try {
      await spotifyTransferPlayback(deviceId);
      await fetchDevices();
    } catch (e) {
      console.error("Failed to transfer playback:", e);
    } finally {
      setTransferring(null);
    }
  };

  return (
    <div className="device-picker" data-no-drag="true">
      <div className="device-picker-header">
        <span>Devices</span>
      </div>
      <div className="device-picker-list">
        {loading ? (
          <div className="device-picker-empty">Loading...</div>
        ) : devices.length === 0 ? (
          <div className="device-picker-empty">
            Device switching is not yet available.
            <br />
            Use Spotify app to switch devices.
          </div>
        ) : (
          devices.map((d) => (
            <button
              key={d.id}
              className={`device-picker-item${d.is_active ? " active" : ""}`}
              onClick={() => !d.is_active && handleTransfer(d.id)}
              disabled={d.is_active || transferring !== null}
            >
              <span className="device-icon">{deviceIcon(d.device_type)}</span>
              <span className="device-name">{d.name}</span>
              {d.is_active && <span className="device-active-dot" />}
              {transferring === d.id && <span className="device-transferring">...</span>}
            </button>
          ))
        )}
      </div>
    </div>
  );
}
