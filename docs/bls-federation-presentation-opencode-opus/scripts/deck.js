/* ===========================================================================
   deck.js — navigation, activation, reveals, progress
   No animation is started here that CSS could not have started itself; this
   file only decides *when* a slide is active and hands that to CSS.
   =========================================================================== */

const html = document.documentElement;
const deck = document.querySelector(".deck");
const slides = [...document.querySelectorAll(".slide")];
const railActs = document.querySelector(".rail__acts");
const mark = document.querySelector(".rail__mark svg");
const progress = document.querySelector(".progress");
const nowEl = document.querySelector("[data-counter-now]");
const totalEl = document.querySelector("[data-counter-total]");

html.classList.add("js");

const reduced = matchMedia("(prefers-reduced-motion: reduce)");

/* --- stagger indices ------------------------------------------------------ */

for (const slide of slides) {
  slide.querySelectorAll("[data-reveal]").forEach((el, i) => {
    if (!el.style.getPropertyValue("--i")) el.style.setProperty("--i", i);
  });
}

/* --- rail ---------------------------------------------------------------- */

const acts = [];
for (const slide of slides) {
  const name = slide.dataset.act || "";
  let act = acts.at(-1);
  if (!act || act.name !== name) {
    act = { name, slides: [] };
    acts.push(act);
  }
  act.slides.push(slide);
}

const dotOf = new Map();

for (const act of acts) {
  const wrap = document.createElement("div");
  wrap.className = "act";
  act.el = wrap;

  const label = document.createElement("div");
  label.className = "act__name";
  label.textContent = act.name;
  wrap.append(label);

  const dots = document.createElement("div");
  dots.className = "act__dots";
  for (const slide of act.slides) {
    const dot = document.createElement("button");
    dot.className = "dot";
    dot.type = "button";
    const n = slides.indexOf(slide) + 1;
    dot.setAttribute(
      "aria-label",
      `Go to slide ${n}: ${slide.dataset.title || slide.id}`,
    );
    dot.addEventListener("click", () => goTo(slides.indexOf(slide)));
    dots.append(dot);
    dotOf.set(slide, dot);
  }
  wrap.append(dots);
  railActs?.append(wrap);
}

if (totalEl) totalEl.textContent = String(slides.length).padStart(2, "0");

/* --- activation ---------------------------------------------------------- */

let current = 0;

function setCurrent(index) {
  if (index === current) return;
  current = index;
  const slide = slides[index];

  for (const s of slides) s.removeAttribute("data-active");
  slide.setAttribute("data-active", "");

  for (const a of acts) a.el.removeAttribute("data-current");
  for (const [s, dot] of dotOf) {
    dot.removeAttribute("data-current");
    if (slides.indexOf(s) <= index) dot.setAttribute("data-seen", "");
  }
  dotOf.get(slide)?.setAttribute("data-current", "");
  acts.find((a) => a.slides.includes(slide))?.el.setAttribute("data-current", "");

  if (nowEl) nowEl.textContent = String(index + 1).padStart(2, "0");

  if (slide.id && location.hash.slice(1) !== slide.id) {
    history.replaceState(null, "", `#${slide.id}`);
  }
}

/* the slide crossing the middle band of the viewport is the current one */
const activeObserver = new IntersectionObserver(
  (entries) => {
    for (const e of entries) {
      if (e.isIntersecting) setCurrent(slides.indexOf(e.target));
    }
  },
  { root: deck, rootMargin: "-49% 0px -49% 0px", threshold: 0 },
);

/* reveal once, on first meaningful appearance — never re-animate on scroll-back */
const seenObserver = new IntersectionObserver(
  (entries) => {
    for (const e of entries) {
      if (e.intersectionRatio > 0.2) {
        e.target.setAttribute("data-seen", "");
        seenObserver.unobserve(e.target);
      }
    }
  },
  { root: deck, threshold: [0, 0.2, 0.5] },
);

for (const slide of slides) {
  activeObserver.observe(slide);
  seenObserver.observe(slide);
}

/* --- navigation ---------------------------------------------------------- */

function goTo(index, instant = false) {
  const i = Math.max(0, Math.min(slides.length - 1, index));
  deck.scrollTo({
    top: slides[i].offsetTop,
    behavior: instant || reduced.matches ? "instant" : "smooth",
  });
}

const KEY_NEXT = new Set(["ArrowDown", "ArrowRight", "PageDown", " ", "j", "n"]);
const KEY_PREV = new Set(["ArrowUp", "ArrowLeft", "PageUp", "k", "p"]);

addEventListener("keydown", (e) => {
  if (e.metaKey || e.ctrlKey || e.altKey) return;
  const tag = e.target?.tagName;
  const typing = tag === "INPUT" || tag === "TEXTAREA" || e.target?.isContentEditable;
  if (typing) return;

  if (KEY_NEXT.has(e.key)) {
    e.preventDefault();
    goTo(current + 1);
  } else if (KEY_PREV.has(e.key)) {
    e.preventDefault();
    goTo(current - 1);
  } else if (e.key === "Home") {
    e.preventDefault();
    goTo(0);
  } else if (e.key === "End") {
    e.preventDefault();
    goTo(slides.length - 1);
  }
});

/* --- progress + plate mark ---------------------------------------------- */

let ticking = false;

function onScroll() {
  if (ticking) return;
  ticking = true;
  requestAnimationFrame(() => {
    ticking = false;
    const max = deck.scrollHeight - deck.clientHeight;
    const p = max > 0 ? deck.scrollTop / max : 0;
    progress?.style.setProperty("--p", p.toFixed(4));
    if (mark && !reduced.matches) {
      mark.style.transform = `rotate(${(p * 210).toFixed(2)}deg)`;
    }
  });
}

deck.addEventListener("scroll", onScroll, { passive: true });
onScroll();

/* --- deep link ---------------------------------------------------------- */

if (location.hash.length > 1) {
  const target = document.getElementById(location.hash.slice(1));
  if (target) {
    requestAnimationFrame(() => goTo(slides.indexOf(target), true));
  }
}

/* mark the first slide immediately so its reveal plays on load */
slides[0].setAttribute("data-active", "");
requestAnimationFrame(() => slides[0].setAttribute("data-seen", ""));

export { goTo, slides };
