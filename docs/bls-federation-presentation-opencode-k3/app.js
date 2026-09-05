/* Deck chrome: scroll-spy, rail nav, keyboard, progress, figure wiring. */
(function () {
  "use strict";

  const deck = document.getElementById("deck");
  const pages = Array.from(document.querySelectorAll(".page"));
  const railNav = document.getElementById("railNav");
  const progressBar = document.getElementById("progressBar");

  /* ── rail nav ─────────────────────────────────────────────── */
  pages.forEach((page, i) => {
    const btn = document.createElement("button");
    btn.className = "rail-link";
    btn.innerHTML = `<span class="n">${String(i).padStart(2, "0")}</span><span>${page.dataset.title}</span>`;
    btn.addEventListener("click", () => goTo(i));
    railNav.appendChild(btn);
  });
  const railLinks = Array.from(railNav.children);

  /* ── page figure map ──────────────────────────────────────── */
  const pageFigures = new Map(); // pageId -> [figureName]
  pages.forEach((page) => {
    const names = Array.from(page.querySelectorAll(".figure-body[id]")).map((el) =>
      el.id.replace(/^fig/, "").replace(/^./, (c) => c.toLowerCase())
    );
    if (names.length) pageFigures.set(page.id, names);
  });

  /* ── navigation ───────────────────────────────────────────── */
  let current = -1;

  function goTo(i) {
    const idx = Math.max(0, Math.min(pages.length - 1, i));
    pages[idx].scrollIntoView({ behavior: "smooth", block: "start" });
  }

  function setActive(idx) {
    if (idx === current) return;
    current = idx;
    railLinks.forEach((l, i) => l.classList.toggle("active", i === idx));
    railLinks[idx].scrollIntoView({ block: "nearest", behavior: "smooth" });
    const id = pages[idx].id;
    history.replaceState(null, "", "#" + id);
    progressBar.style.width = `${(idx / (pages.length - 1)) * 100}%`;

    // reveal + figure autoplay
    pages[idx].classList.add("seen");
    const figs = pageFigures.get(id) || [];
    figs.forEach((name) => Figures.autoplay(name));
  }

  const io = new IntersectionObserver(
    (entries) => {
      entries.forEach((e) => {
        if (e.isIntersecting) setActive(pages.indexOf(e.target));
      });
    },
    { root: deck, threshold: 0.55 }
  );
  pages.forEach((p) => io.observe(p));

  /* ── keyboard ─────────────────────────────────────────────── */
  const stepCache = new Map(); // figureName -> currentStep

  function activeFigures() {
    return pageFigures.get(pages[current] && pages[current].id) || [];
  }

  window.addEventListener("keydown", (e) => {
    if (e.target.matches("input, textarea, select")) return;
    const figs = activeFigures();
    switch (e.key) {
      case "ArrowDown":
      case "PageDown":
      case "j":
      case " ":
        e.preventDefault();
        goTo(current + 1);
        break;
      case "ArrowUp":
      case "PageUp":
      case "k":
        e.preventDefault();
        goTo(current - 1);
        break;
      case "Home":
        e.preventDefault();
        goTo(0);
        break;
      case "End":
        e.preventDefault();
        goTo(pages.length - 1);
        break;
      case "ArrowRight": {
        if (!figs.length) break;
        e.preventDefault();
        const name = figs[0];
        const next = (stepCache.get(name) ?? Figures.stepCount(name) - 1) + 1;
        stepCache.set(name, Figures.step(name, next));
        break;
      }
      case "ArrowLeft": {
        if (!figs.length) break;
        e.preventDefault();
        const name = figs[0];
        const prev = (stepCache.get(name) ?? 0) - 1;
        stepCache.set(name, Figures.step(name, prev));
        break;
      }
      case "r":
      case "R":
        if (!figs.length) break;
        figs.forEach((name) => stepCache.set(name, Figures.play(name)));
        break;
    }
  });

  Figures.onStepChange((name, idx) => stepCache.set(name, idx));

  /* ── deep link ────────────────────────────────────────────── */
  function applyHash() {
    const id = location.hash.slice(1);
    const idx = pages.findIndex((p) => p.id === id);
    if (idx >= 0) {
      pages[idx].scrollIntoView({ block: "start", behavior: "auto" });
      setActive(idx);
    } else {
      setActive(0);
    }
  }

  Figures.init();
  applyHash();
  window.addEventListener("hashchange", applyHash);
})();
