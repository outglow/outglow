// The site's menu on a phone: the button opens and closes the links (CSS
// folds them into it below 48rem); Escape and a chosen link close it.
const header = document.querySelector<HTMLElement>("header.site");
const button = header?.querySelector<HTMLButtonElement>("button.menu");
if (header && button) {
  const set = (open: boolean) => {
    header.classList.toggle("open", open);
    button.setAttribute("aria-expanded", String(open));
  };
  button.addEventListener("click", () => set(!header.classList.contains("open")));
  header.querySelectorAll("nav a").forEach((a) => a.addEventListener("click", () => set(false)));
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && header.classList.contains("open")) {
      set(false);
      button.focus();
    }
  });
}
