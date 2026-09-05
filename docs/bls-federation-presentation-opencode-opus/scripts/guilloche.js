/* ===========================================================================
   guilloche.js — the signature
   Security-print rosettes are hypotrochoids: the same parametric family as the
   polynomial and curve plots in this deck. The plate draws itself once.
   =========================================================================== */

const TAU = Math.PI * 2;

function gcd(a, b) {
  return b ? gcd(b, a % b) : a;
}

/** One hypotrochoid, normalised so its extreme radius is `radius`. */
function rosettePath({ R, r, d, radius, rotate = 0, samples = 720 }) {
  const k = R - r;
  const ratio = k / r;
  const turns = r / gcd(R, r);
  const extent = k + d;
  const s = radius / extent;
  const pts = [];
  const total = Math.round(samples * turns);
  for (let i = 0; i <= total; i++) {
    const t = (i / total) * TAU * turns;
    const x = k * Math.cos(t) + d * Math.cos(ratio * t);
    const y = k * Math.sin(t) - d * Math.sin(ratio * t);
    const ca = Math.cos(rotate);
    const sa = Math.sin(rotate);
    pts.push([(x * ca - y * sa) * s, (x * sa + y * ca) * s]);
  }
  return (
    "M" +
    pts.map(([x, y]) => `${x.toFixed(2)} ${y.toFixed(2)}`).join("L") +
    "Z"
  );
}

const BANDS = {
  hero: [
    { R: 8, r: 3, d: 5, radius: 100, copies: 20, color: "var(--g1)", w: 0.55, o: 0.5 },
    { R: 7, r: 4, d: 6, radius: 74, copies: 14, color: "var(--g2)", w: 0.55, o: 0.55 },
    { R: 5, r: 2, d: 4, radius: 48, copies: 10, color: "var(--wallet)", w: 0.5, o: 0.4 },
    { R: 9, r: 2, d: 3, radius: 26, copies: 7, color: "var(--secp)", w: 0.5, o: 0.55 },
  ],
  mark: [
    { R: 8, r: 3, d: 5, radius: 46, copies: 8, color: "var(--g1)", w: 1.1, o: 0.6 },
    { R: 5, r: 2, d: 4, radius: 24, copies: 5, color: "var(--g2)", w: 1.1, o: 0.6 },
  ],
  seal: [
    { R: 7, r: 4, d: 6, radius: 84, copies: 12, color: "var(--g2)", w: 0.6, o: 0.5 },
    { R: 8, r: 3, d: 5, radius: 54, copies: 9, color: "var(--g1)", w: 0.6, o: 0.5 },
  ],
};

function draw(el) {
  const preset = el.dataset.guilloche || "mark";
  const bands = BANDS[preset] ?? BANDS.mark;
  const box = preset === "mark" ? 56 : 110;
  const ns = "http://www.w3.org/2000/svg";

  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", `${-box} ${-box} ${box * 2} ${box * 2}`);
  svg.setAttribute("class", "guilloche" + (preset === "mark" ? " guilloche--static" : ""));
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");

  let i = 0;

  // guard rings
  const rings = preset === "hero" ? [104, 101, 90] : preset === "seal" ? [92, 89] : [50];
  for (const r of rings) {
    const c = document.createElementNS(ns, "circle");
    c.setAttribute("r", String(r));
    c.setAttribute("fill", "none");
    c.setAttribute("stroke", "var(--line)");
    c.setAttribute("stroke-width", "0.6");
    svg.append(c);
  }

  for (const band of bands) {
    const g = document.createElementNS(ns, "g");
    g.setAttribute("stroke", band.color);
    g.setAttribute("stroke-width", String(band.w));
    g.setAttribute("opacity", String(band.o));
    for (let c = 0; c < band.copies; c++) {
      const p = document.createElementNS(ns, "path");
      p.setAttribute(
        "d",
        rosettePath({
          R: band.R,
          r: band.r,
          d: band.d,
          radius: band.radius,
          rotate: (TAU * c) / band.copies / (band.R - band.r),
          samples: preset === "mark" ? 240 : 480,
        }),
      );
      p.setAttribute("pathLength", "100");
      p.style.setProperty("--i", String(i++ % 24));
      g.append(p);
    }
    svg.append(g);
  }

  el.replaceChildren(svg);
}

for (const el of document.querySelectorAll("[data-guilloche]")) draw(el);
