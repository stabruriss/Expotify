interface ReadStartChecks {
  /** Whether Spotify is playing on this Mac; null when that cannot be told. */
  playbackOnThisMac: () => Promise<boolean | null>;
  /** Whether the read's track is still playing and the read is still current. */
  trackIsCurrent: () => Promise<boolean>;
}

/**
 * Whether a read may pause the music and speak.
 *
 * With one account on two Macs, the idle Mac's Spotify mirrors the other Mac's playback, so an
 * automatic read there would speak on the wrong Mac and pause the music on the other. Automatic
 * reads are skipped while another device plays; when that cannot be told, they go ahead as before.
 * The device check is a network round trip, so the track is checked after it, last before the
 * music is paused: a track change during the check must not let the old read through.
 */
export async function readMayStart(autoTriggered: boolean, checks: ReadStartChecks): Promise<boolean> {
  if (autoTriggered && (await playingOnAnotherDevice(checks.playbackOnThisMac))) return false;
  return checks.trackIsCurrent();
}

async function playingOnAnotherDevice(playbackOnThisMac: () => Promise<boolean | null>): Promise<boolean> {
  try {
    return (await playbackOnThisMac()) === false;
  } catch (e) {
    console.error("Failed to check where Spotify is playing:", e);
    return false;
  }
}
