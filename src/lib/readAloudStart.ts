interface ReadStartChecks {
  /** Whether Spotify is playing on this Mac; null when that cannot be told. */
  playbackOnThisMac: () => Promise<boolean | null>;
  /** Whether the read's track is still playing and the read is still current. */
  trackIsCurrent: () => Promise<boolean>;
}

/**
 * Whether a read may pause the music and speak.
 *
 * Reads speak on this Mac wherever the music plays: a speaker, a phone or another Mac. With one
 * account on several Macs, a Mac can opt out of automatic reads while another device plays
 * (`skipWhenPlayingElsewhere`); when that cannot be told, the read goes ahead.
 * The device check is a network round trip, so the track is checked after it, last before the
 * music is paused: a track change during the check must not let the old read through.
 */
export async function readMayStart(skipWhenPlayingElsewhere: boolean, checks: ReadStartChecks): Promise<boolean> {
  if (skipWhenPlayingElsewhere && (await playingOnAnotherDevice(checks.playbackOnThisMac))) return false;
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
