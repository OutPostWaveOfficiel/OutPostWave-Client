// Notification sounds and taskbar badge.
import { api } from "./api";

let audio: AudioContext | null = null;
let lastChime = 0;

// WebViews may refuse to start audio before a user gesture: create the context on the first one.
function unlockAudio() {
  try {
    audio ??= new AudioContext();
    void audio.resume();
  } catch {
    // No audio device: stay silent.
  }
}
window.addEventListener("pointerdown", unlockAudio, { once: true });
window.addEventListener("keydown", unlockAudio, { once: true });

/** Short synthesized chime (no audio asset). `message`: two rising notes; `activity`: softer. */
export function chime(kind: "message" | "activity") {
  const now = Date.now();
  if (now - lastChime < 1200) return; // A burst of messages plays a single chime.
  lastChime = now;
  try {
    audio ??= new AudioContext();
    void audio.resume();
    const ctx = audio;
    const notes = kind === "message" ? [880, 1318.5] : [659.25, 987.77];
    const volume = kind === "message" ? 0.16 : 0.1;
    notes.forEach((freq, i) => {
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      const t0 = ctx.currentTime + i * 0.11;
      osc.type = "sine";
      osc.frequency.value = freq;
      gain.gain.setValueAtTime(0, t0);
      gain.gain.linearRampToValueAtTime(volume, t0 + 0.012);
      gain.gain.exponentialRampToValueAtTime(0.0001, t0 + 0.38);
      osc.connect(gain).connect(ctx.destination);
      osc.start(t0);
      osc.stop(t0 + 0.4);
    });
  } catch {
    // Ignore audio errors.
  }
}

/** Red 32×32 badge with the count, as RGBA bytes (Windows taskbar overlay). */
function badgePixels(count: number): number[] | null {
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 32;
  const g = canvas.getContext("2d");
  if (!g) return null;
  g.fillStyle = "#e5383b";
  g.beginPath();
  g.arc(16, 16, 15.5, 0, Math.PI * 2);
  g.fill();
  const text = count > 99 ? "99+" : String(count);
  g.fillStyle = "#fff";
  g.font = `bold ${text.length > 2 ? 12 : text.length > 1 ? 16 : 20}px system-ui, sans-serif`;
  g.textAlign = "center";
  g.textBaseline = "middle";
  g.fillText(text, 16, 17);
  return Array.from(g.getImageData(0, 0, 32, 32).data);
}

let shownCount = -1;

/** Shows the unread count on the taskbar icon (no badge at 0). */
export function setBadge(count: number, os: string) {
  if (count === shownCount) return;
  shownCount = count;
  const overlay = os === "windows" && count > 0 ? badgePixels(count) : null;
  void api.setBadge(count, overlay).catch(() => {});
}
