import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, catalog } from "../api";

export type VoicePreviewState = "idle" | "loading" | "playing";

/** Manages on-demand TTS previews for the voice pickers.
 *
 * Caches the synthesised WAV per voice id (as an object URL) so a second
 * play doesn't re-hit the TTS API. Only one voice plays at a time. */
export function useVoicePreview(): {
  stateFor: (voiceId: string) => VoicePreviewState;
  toggle: (voiceId: string) => void;
  error: string | null;
} {
  const [loadingId, setLoadingId] = useState<string | null>(null);
  const [playingId, setPlayingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const cacheRef = useRef<Map<string, string>>(new Map());

  const stop = useCallback((): void => {
    const a = audioRef.current;
    if (a) {
      a.pause();
      a.currentTime = 0;
    }
    audioRef.current = null;
    setPlayingId(null);
  }, []);

  const play = useCallback(
    (voiceId: string, src: string): void => {
      stop();
      const a = new Audio(src);
      audioRef.current = a;
      a.onended = () => {
        if (audioRef.current === a) {
          audioRef.current = null;
          setPlayingId(null);
        }
      };
      a.onerror = () => {
        if (audioRef.current === a) {
          audioRef.current = null;
          setPlayingId(null);
          setError("Could not play preview");
        }
      };
      setPlayingId(voiceId);
      void a.play().catch(() => {
        if (audioRef.current === a) {
          audioRef.current = null;
          setPlayingId(null);
        }
      });
    },
    [stop],
  );

  const toggle = useCallback(
    (voiceId: string): void => {
      setError(null);
      if (playingId === voiceId) {
        stop();
        return;
      }
      const cached = cacheRef.current.get(voiceId);
      if (cached) {
        play(voiceId, cached);
        return;
      }
      if (loadingId) return;
      setLoadingId(voiceId);
      catalog
        .previewVoice(voiceId)
        .then((res) => {
          const buf = base64ToArrayBuffer(res.audio_wav_base64);
          const blob = new Blob([buf], { type: "audio/wav" });
          const url = URL.createObjectURL(blob);
          cacheRef.current.set(voiceId, url);
          setLoadingId((cur) => (cur === voiceId ? null : cur));
          play(voiceId, url);
        })
        .catch((err: unknown) => {
          setLoadingId((cur) => (cur === voiceId ? null : cur));
          setError(
            err instanceof ApiError ? err.message : "Could not load preview",
          );
        });
    },
    [loadingId, play, playingId, stop],
  );

  useEffect(() => {
    const cache = cacheRef.current;
    return () => {
      const a = audioRef.current;
      if (a) {
        a.pause();
        a.currentTime = 0;
      }
      audioRef.current = null;
      for (const url of cache.values()) URL.revokeObjectURL(url);
      cache.clear();
    };
  }, []);

  const stateFor = (voiceId: string): VoicePreviewState => {
    if (playingId === voiceId) return "playing";
    if (loadingId === voiceId) return "loading";
    return "idle";
  };

  return { stateFor, toggle, error };
}

function base64ToArrayBuffer(b64: string): ArrayBuffer {
  const bin = atob(b64);
  const buf = new ArrayBuffer(bin.length);
  const view = new Uint8Array(buf);
  for (let i = 0; i < bin.length; i++) view[i] = bin.charCodeAt(i);
  return buf;
}
