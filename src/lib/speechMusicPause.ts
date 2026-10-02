type PlaybackState = { is_playing: boolean } | null;

interface MusicCommands {
  getTrack: () => Promise<PlaybackState>;
  pause: () => Promise<void>;
  play: () => Promise<void>;
}

/** Insight and Chat share one pause; only music paused by speech is resumed. */
export class SpeechMusicPause {
  private owners = new Set<symbol>();
  private shouldResume = false;
  private pending = Promise.resolve();

  constructor(private commands: MusicCommands) {}

  private run(operation: () => Promise<void>): Promise<void> {
    const result = this.pending.then(operation);
    // A failed command must not block subsequent speech sessions.
    this.pending = result.catch(() => {});
    return result;
  }

  hold(owner: symbol): Promise<void> {
    this.owners.add(owner);
    return this.run(async () => {
      if (!this.owners.has(owner) || this.shouldResume) return;
      // Read after earlier pause/resume commands finish; a caller's snapshot
      // could still say paused while the previous reader is restoring music.
      const track = await this.commands.getTrack();
      if (!this.owners.has(owner) || !track?.is_playing) return;
      await this.commands.pause();
      // release() queues behind this command, including cancellation mid-pause.
      this.shouldResume = true;
    }).catch((error) => {
      this.owners.delete(owner);
      throw error;
    });
  }

  release(owner: symbol): Promise<void> {
    if (!this.owners.delete(owner)) return Promise.resolve();
    return this.run(async () => {
      if (this.owners.size > 0 || !this.shouldResume) return;
      this.shouldResume = false;
      await this.commands.play();
    });
  }
}
