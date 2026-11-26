// Canvas waveform for the overlay: mirrored bars that slide left, newest on the right.
// It draws with requestAnimationFrame and reads a level ref, so audio-level events never
// cause a React render.
import { useEffect, useRef } from 'react';
import type { MutableRefObject } from 'react';
import * as api from '../api';
import { meterLevel } from '../store';

const BAR_W = 3;
const BAR_GAP = 3;
const PITCH = BAR_W + BAR_GAP;
const SLOT_H = 28;
const MIN_H = 3;
/** One new bar per this many ms. The backend sends levels at about 20 Hz. */
const SAMPLE_MS = 45;
/** With no level event for this long, the level reads as silence. */
const STALE_MS = 500;

/**
 * Latest 0..1 level for the overlay. `both` takes the louder of microphone and system
 * (live modes); otherwise only the microphone counts (dictation).
 */
export function useLevelRef(both: boolean): MutableRefObject<number> {
  const level = useRef(0);
  const bothRef = useRef(both);
  bothRef.current = both;

  useEffect(() => {
    const last = { microphone: 0, system: 0 };
    let lastEvent = 0;
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    api
      .onAudioLevel((p) => {
        last[p.source] = meterLevel(p.rms);
        lastEvent = performance.now();
        level.current = bothRef.current ? Math.max(last.microphone, last.system) : last.microphone;
      })
      .then((off) => {
        if (cancelled) off();
        else unlisten = off;
      })
      .catch(() => {});

    const stale = window.setInterval(() => {
      if (performance.now() - lastEvent > STALE_MS) {
        last.microphone = 0;
        last.system = 0;
        level.current = 0;
      }
    }, 150);

    return () => {
      cancelled = true;
      unlisten?.();
      window.clearInterval(stale);
    };
  }, []);

  return level;
}

export function Waveform({
  levelRef,
  frozen,
  reducedMotion,
}: {
  /** Mutable ref holding the latest 0..1 level. Read every animation frame. */
  levelRef: MutableRefObject<number>;
  /** Stop adding bars. The canvas keeps what it has drawn. */
  frozen: boolean;
  reducedMotion: boolean;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const frozenRef = useRef(frozen);
  frozenRef.current = frozen;

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext('2d');
    if (!canvas || !ctx) return;

    let width = canvas.clientWidth || 200;
    let n = Math.max(1, Math.floor((width + BAR_GAP) / PITCH));
    let ring: number[] = new Array(n).fill(0);
    let env = 0;
    let acc = 0;
    let prev = performance.now();
    let color = '#ededf0';
    let frame = 0;

    // The bar color comes from the canvas's CSS `color`, so the stylesheet decides it.
    const readColor = () => {
      const c = getComputedStyle(canvas).color;
      if (c) color = c;
    };

    const resize = () => {
      width = canvas.clientWidth || 200;
      const next = Math.max(1, Math.floor((width + BAR_GAP) / PITCH));
      const old = ring;
      ring = new Array(next).fill(0);
      for (let i = 0; i < Math.min(next, old.length); i++) ring[next - 1 - i] = old[old.length - 1 - i];
      n = next;
      const dpr = window.devicePixelRatio || 1;
      canvas.width = Math.round(width * dpr);
      canvas.height = Math.round(SLOT_H * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };
    resize();
    readColor();
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);

    const barHeight = (v: number) => MIN_H + (SLOT_H - MIN_H) * Math.pow(Math.min(1, Math.max(0, v)), 1.5);
    const drawBar = (x: number, h: number) => {
      ctx.beginPath();
      ctx.roundRect(x, (SLOT_H - h) / 2, BAR_W, h, 1.5);
      ctx.fill();
    };

    let raf = 0;
    const tick = (now: number) => {
      const dt = Math.min(250, now - prev);
      prev = now;
      if (frame++ % 30 === 0) readColor();

      const target = Math.min(1, Math.max(0, levelRef.current));

      if (!frozenRef.current) {
        env += (target - env) * (target > env ? 0.6 : 0.15);
        if (!reducedMotion) {
          acc += dt;
          while (acc >= SAMPLE_MS) {
            ring.shift();
            ring.push(env);
            acc -= SAMPLE_MS;
          }
        }
      }

      ctx.clearRect(0, 0, width, SLOT_H);
      ctx.fillStyle = color;

      if (reducedMotion) {
        // Static bars at the current level, shaped by a fixed envelope. Nothing slides.
        for (let i = 0; i < n; i++) {
          const shape = 0.35 + 0.65 * Math.abs(Math.sin(i * 0.9));
          drawBar(i * PITCH, barHeight(env * shape));
        }
      } else {
        const offset = frozenRef.current ? 0 : -(acc / SAMPLE_MS) * PITCH;
        for (let i = 0; i < n; i++) {
          // The newest bar follows the live envelope instead of the last pushed sample.
          const v = i === n - 1 && !frozenRef.current ? env : ring[i];
          drawBar(i * PITCH + offset, barHeight(v));
        }
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);

    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  }, [levelRef, reducedMotion]);

  return <canvas ref={canvasRef} className="ov-wave" aria-hidden="true" />;
}
