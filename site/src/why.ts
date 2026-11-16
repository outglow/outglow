// The comparison page: each row's "i" opens its explanation.

for (const b of document.querySelectorAll<HTMLButtonElement>("button.info")) {
  b.addEventListener("click", () => {
    const more = document.getElementById(b.getAttribute("aria-controls")!)!;
    const open = b.getAttribute("aria-expanded") !== "true";
    b.setAttribute("aria-expanded", String(open));
    more.hidden = !open;
  });
}
