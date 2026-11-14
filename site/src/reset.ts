// The factory reset sequence on the install page: a timeline drawn to scale and
// a guide that says when to switch the bulb on and off.

type Step = { on: boolean; s: number; label: string };

/** 1 s on three times, 12 s on twice, off about 6 s between; then on to stay. */
export const SEQUENCE: Step[] = [
  { on: true, s: 1, label: "On" }, { on: false, s: 6, label: "Off" },
  { on: true, s: 1, label: "On" }, { on: false, s: 6, label: "Off" },
  { on: true, s: 1, label: "On" }, { on: false, s: 6, label: "Off" },
  { on: true, s: 12, label: "On" }, { on: false, s: 6, label: "Off" },
  { on: true, s: 12, label: "On" }, { on: false, s: 6, label: "Off" },
];

function timeline(host: HTMLElement): HTMLElement[] {
  const segs = SEQUENCE.map((st) => {
    const d = document.createElement("div");
    d.className = `seg ${st.on ? "on" : "off"}`;
    d.style.flexGrow = String(st.s);
    d.textContent = st.on ? `${st.s} s` : "off";
    return d;
  });
  const last = document.createElement("div");
  last.className = "seg on stay";
  last.style.flexGrow = "8";
  last.textContent = "on ✓";
  host.replaceChildren(...segs, last);
  return [...segs, last];
}

function main(): void {
  const host = document.getElementById("reset-timeline");
  const start = document.getElementById("reset-start") as HTMLButtonElement | null;
  const now = document.getElementById("reset-now");
  if (!host || !start || !now) return;
  const segs = timeline(host);
  let timer = 0;

  const stop = (text?: string) => {
    window.clearInterval(timer);
    timer = 0;
    segs.forEach((s) => s.classList.remove("active", "done"));
    start.textContent = "Guide me through it";
    if (text) now.textContent = text;
    else now.hidden = true;
  };

  start.addEventListener("click", () => {
    if (timer) return stop();
    start.textContent = "Stop";
    now.hidden = false;
    let i = -1;
    let left = 3;
    const show = () => {
      segs.forEach((s, k) => {
        s.classList.toggle("active", k === i);
        s.classList.toggle("done", k < i);
      });
      if (i < 0) {
        now.innerHTML = `<strong>Get ready</strong><span>Start with the bulb switched off · ${left}</span>`;
      } else {
        const st = SEQUENCE[i];
        if (!st) return;
        now.className = `reset-now ${st.on ? "on" : "off"}`;
        now.innerHTML = `<strong>Switch ${st.on ? "ON" : "OFF"}</strong><span>${left} s</span>`;
      }
    };
    show();
    timer = window.setInterval(() => {
      left -= 1;
      if (left > 0) return show();
      i += 1;
      if (i >= SEQUENCE.length) {
        segs.forEach((s, k) => s.classList.toggle("done", k < segs.length - 1));
        segs[segs.length - 1]?.classList.add("active");
        now.className = "reset-now on";
        now.innerHTML = "<strong>Switch ON and leave it on</strong><span>It blinks three times when the reset worked.</span>";
        window.clearInterval(timer);
        timer = 0;
        start.textContent = "Start again";
        return;
      }
      left = SEQUENCE[i]?.s ?? 0;
      show();
    }, 1000);
  });
}

main();
