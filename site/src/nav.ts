// The site's menu on a phone: the button opens and closes the links (CSS
// folds them into it below 48rem); Escape and a chosen link close it.
const header = document.querySelector<HTMLElement>("header.site");
const button = header?.querySelector<HTMLButtonElement>("button.menu");
if (header && button) {
  const groups = [...header.querySelectorAll<HTMLDetailsElement>(".nav-group")];
  const closeGroups = () => groups.forEach((group) => { group.open = false; });
  const set = (open: boolean) => {
    header.classList.toggle("open", open);
    button.setAttribute("aria-expanded", String(open));
    if (!open) closeGroups();
  };
  groups.forEach((group) => group.addEventListener("toggle", () => {
    if (group.open) groups.forEach((other) => { if (other !== group) other.open = false; });
  }));
  document.addEventListener("click", (event) => {
    if (!header.contains(event.target as Node)) closeGroups();
  });
  button.addEventListener("click", () => set(!header.classList.contains("open")));
  header.querySelectorAll("nav a").forEach((a) => a.addEventListener("click", () => set(false)));
  document.addEventListener("keydown", (e) => {
    const group = groups.find((item) => item.open);
    if (e.key === "Escape" && group) {
      group.open = false;
      group.querySelector("summary")!.focus();
      return;
    }
    if (e.key === "Escape" && header.classList.contains("open")) {
      set(false);
      button.focus();
    }
  });
}
