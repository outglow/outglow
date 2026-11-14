// The flashing illustration: a filament bulb that fills with light as the
// image goes over, and glows when the bulb has taken it. Original artwork.

const SVG = "http://www.w3.org/2000/svg";

const MARKUP = `
<svg viewBox="0 0 120 180" role="img" aria-label="Bulb being updated" class="bulb-art">
  <defs>
    <clipPath id="bulb-glass">
      <path d="M60 8C33 8 14 28 14 54c0 17 8 29 17 39 6 7 10 14 10 22v9h38v-9c0-8 4-15 10-22 9-10 17-22 17-39C106 28 87 8 60 8Z"/>
    </clipPath>
    <radialGradient id="bulb-glow" cx="50%" cy="40%" r="60%">
      <stop offset="0" stop-color="var(--glow-core)"/>
      <stop offset="1" stop-color="var(--glow-edge)"/>
    </radialGradient>
    <filter id="bulb-blur" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="8"/></filter>
  </defs>
  <circle class="halo" cx="60" cy="56" r="46" fill="var(--glow-core)" filter="url(#bulb-blur)"/>
  <g clip-path="url(#bulb-glass)">
    <rect x="0" y="0" width="120" height="140" fill="var(--glass)"/>
    <rect class="fill" x="0" y="8" width="120" height="140" fill="url(#bulb-glow)"/>
  </g>
  <path d="M60 8C33 8 14 28 14 54c0 17 8 29 17 39 6 7 10 14 10 22v9h38v-9c0-8 4-15 10-22 9-10 17-22 17-39C106 28 87 8 60 8Z"
        fill="none" stroke="var(--outline)" stroke-width="3"/>
  <path class="filament" d="M48 110V74l6-10 6 10 6-10 6 10v36" fill="none" stroke="var(--filament)" stroke-width="2.5" stroke-linejoin="round"/>
  <g fill="var(--base)">
    <rect x="40" y="128" width="40" height="8" rx="3"/>
    <rect x="42" y="139" width="36" height="7" rx="3"/>
    <rect x="44" y="149" width="32" height="7" rx="3"/>
    <path d="M50 159h20l-4 9h-12z"/>
  </g>
</svg>`;

export class BulbArt {
  private readonly root: HTMLElement;
  private readonly fill: SVGRectElement;

  constructor(host: HTMLElement) {
    host.innerHTML = MARKUP;
    this.root = host;
    this.fill = host.querySelector<SVGRectElement>("rect.fill")!;
    if (!(this.fill instanceof SVGElement) || this.fill.namespaceURI !== SVG) throw new Error("bulb art");
    this.set(0, "idle");
  }

  /** progress 0..1; state: idle, busy, done, failed */
  set(progress: number, state: "idle" | "busy" | "done" | "failed"): void {
    const p = Math.max(0, Math.min(1, progress));
    // the glass spans y 8..124: the light rises from the bottom
    this.fill.style.transform = `translateY(${(116 * (1 - p)).toFixed(1)}px)`;
    this.root.dataset.state = state;
    this.root.style.setProperty("--level", p.toFixed(3));
  }
}
