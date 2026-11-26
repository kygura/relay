// Player for the recording kept with a History entry: play or pause, elapsed and total time,
// and a seek bar. The file loads on the first play, and only one clip plays at a time.

import { useEffect, useRef, useState } from 'react';
import * as api from '../api';
import { IconPause, IconPlay } from '../icons';
import { reportError } from '../store';

let playing: HTMLAudioElement | null = null;

function clock(s: number): string {
  if (!Number.isFinite(s) || s < 0) s = 0;
  const t = Math.floor(s);
  const m = Math.floor(t / 60);
  return `${m}:${String(t % 60).padStart(2, '0')}`;
}

export function AudioClip({
  file,
  durationMs,
  tabIndex,
}: {
  file: string;
  durationMs: number | null;
  tabIndex: number;
}) {
  const audio = useRef<HTMLAudioElement | null>(null);
  const [on, setOn] = useState(false);
  const [pos, setPos] = useState(0);
  const [len, setLen] = useState((durationMs ?? 0) / 1000);
  const [failed, setFailed] = useState(false);

  // Stop and release the clip when the row goes away.
  useEffect(
    () => () => {
      const a = audio.current;
      if (a) {
        a.pause();
        if (playing === a) playing = null;
        a.removeAttribute('src');
        a.load();
      }
    },
    [],
  );

  const load = async (): Promise<HTMLAudioElement> => {
    if (audio.current) return audio.current;
    const a = new Audio();
    a.preload = 'auto';
    a.addEventListener('timeupdate', () => setPos(a.currentTime));
    a.addEventListener('loadedmetadata', () => {
      if (Number.isFinite(a.duration) && a.duration > 0) setLen(a.duration);
    });
    a.addEventListener('play', () => setOn(true));
    a.addEventListener('pause', () => setOn(false));
    a.addEventListener('ended', () => {
      setOn(false);
      setPos(0);
      a.currentTime = 0;
    });
    a.addEventListener('error', () => {
      setOn(false);
      setFailed(true);
    });
    a.src = await api.historyAudioUrl(file);
    audio.current = a;
    return a;
  };

  const toggle = async () => {
    try {
      const a = await load();
      if (!a.paused) {
        a.pause();
        return;
      }
      if (playing && playing !== a) playing.pause();
      playing = a;
      await a.play();
    } catch (e) {
      setFailed(true);
      reportError('audio', e);
    }
  };

  const seek = async (to: number) => {
    setPos(to);
    try {
      const a = await load();
      a.currentTime = to;
    } catch (e) {
      reportError('audio', e);
    }
  };

  if (failed) {
    return (
      <div className="h-audio failed" role="group" aria-label="Recording">
        <span className="muted">The recording could not be played. The file may have been moved or deleted.</span>
      </div>
    );
  }

  const max = Math.max(len, 0.1);
  return (
    <div className={`h-audio${on ? ' on' : ''}`} role="group" aria-label="Recording">
      <button
        type="button"
        className="h-play"
        tabIndex={tabIndex}
        aria-label={on ? 'Pause recording' : 'Play recording'}
        title={on ? 'Pause' : 'Play'}
        onClick={() => void toggle()}
      >
        {on ? <IconPause size={13} /> : <IconPlay size={13} />}
      </button>
      <input
        type="range"
        className="h-seek"
        min={0}
        max={max}
        step={0.1}
        value={Math.min(pos, max)}
        tabIndex={tabIndex}
        aria-label="Position in recording"
        aria-valuetext={`${clock(pos)} of ${clock(len)}`}
        style={{ ['--p' as string]: `${(Math.min(pos, max) / max) * 100}%` }}
        onChange={(e) => void seek(Number(e.target.value))}
      />
      <span className="h-clock" aria-hidden="true">
        {clock(pos)} / {clock(len)}
      </span>
    </div>
  );
}
