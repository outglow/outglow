// Section navigation stays beside long documents; phones use a disclosure.
const menu = document.querySelector<HTMLDetailsElement>(".page-menu");
const main = document.querySelector<HTMLElement>(".doc-layout main");
if (menu && main) {
  const desktop = matchMedia("(min-width: 72rem)");
  const summary = menu.querySelector("summary")!;
  const links = [...menu.querySelectorAll<HTMLAnchorElement>(".toc a")];
  const sections = links.map((link) => document.getElementById(link.hash.slice(1))!);
  const layout = () => { menu.open = desktop.matches; };
  layout();
  desktop.addEventListener("change", layout);
  for (const heading of [document.getElementById("page-top")!, ...sections]) heading.tabIndex = -1;
  menu.querySelectorAll<HTMLAnchorElement>("a").forEach((link) => {
    link.addEventListener("click", () => { if (!desktop.matches) menu.open = false; });
  });
  menu.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !desktop.matches && menu.open) {
      menu.open = false;
      summary.focus();
    }
  });
  let pending = false;
  const update = () => {
    pending = false;
    let current = -1;
    for (let i = 0; i < sections.length; i++) {
      if (sections[i]!.getBoundingClientRect().top <= 120) current = i;
    }
    if (window.scrollY > 0 && window.scrollY + window.innerHeight >= document.documentElement.scrollHeight - 2) {
      current = sections.length - 1;
    }
    links.forEach((link, index) => {
      if (index === current) link.setAttribute("aria-current", "location");
      else link.removeAttribute("aria-current");
    });
  };
  const schedule = () => {
    if (!pending) { pending = true; requestAnimationFrame(update); }
  };
  window.addEventListener("scroll", schedule, { passive: true });
  window.addEventListener("resize", schedule);
  new ResizeObserver(schedule).observe(main);
  update();
}
