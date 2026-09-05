/* ===========================================================================
   figures.js — the interactive plates
   Each figure builds its DOM once and then only flips attributes, so every
   transition is a CSS transition and stays interruptible.
   =========================================================================== */

const NS = "http://www.w3.org/2000/svg";
const reduced = matchMedia("(prefers-reduced-motion: reduce)");

/** Parse an SVG source string into a real, namespaced <svg> element. */
function makeSvg(viewBox, inner, label) {
  const t = document.createElement("template");
  t.innerHTML = `<svg class="svg" viewBox="${viewBox}" role="img" aria-label="${label}">${inner}</svg>`;
  return t.content.firstElementChild;
}

/** Run `cb` the first time the figure's slide becomes the current one. */
function onFirstActive(el, cb) {
  const slide = el.closest(".slide");
  if (!slide) return cb();
  if (slide.hasAttribute("data-active")) return cb();
  const mo = new MutationObserver(() => {
    if (slide.hasAttribute("data-active")) {
      mo.disconnect();
      cb();
    }
  });
  mo.observe(slide, { attributes: true, attributeFilter: ["data-active"] });
}

/** Small stable PRNG so "random" bytes are identical on every reload. */
function rng(seed) {
  let s = seed >>> 0;
  return () => {
    s ^= s << 13;
    s ^= s >>> 17;
    s ^= s << 5;
    s >>>= 0;
    return s / 0x100000000;
  };
}

function hexOf(seed, bytes = 32) {
  const r = rng(seed);
  let out = "";
  for (let i = 0; i < bytes; i++) {
    out += Math.floor(r() * 256)
      .toString(16)
      .padStart(2, "0");
  }
  return out;
}

/* =========================================================================
   08 · rejection sampler
   ========================================================================= */

function initSampler(root) {
  const list = root.querySelector("[data-sampler]");
  const replay = root.querySelector("[data-sampler-replay]");
  if (!list) return;

  const script = [
    {
      hex: "ffffffff00000001000000000000000000000000000000000000000000000000",
      verdict: "reject",
      why: "≥ r · not canonical",
    },
    {
      hex: "0000000000000000000000000000000000000000000000000000000000000000",
      verdict: "reject",
      why: "all zero",
    },
    { hex: hexOf(0x5eed1), verdict: "reject", why: "≥ r · not canonical" },
    { hex: hexOf(0xc0ffee), verdict: "accept", why: "canonical Fr*" },
  ];

  function header() {
    const row = document.createElement("div");
    row.className = "sampler__row";
    row.innerHTML =
      `<span>attempt</span><span class="sampler__hex">HMAC-SHA256(seed, msg ‖ u32_BE(attempt))</span>` +
      `<span class="sampler__v">verdict</span>`;
    return row;
  }

  let timers = [];

  function run() {
    for (const t of timers) clearTimeout(t);
    timers = [];
    list.replaceChildren(header());

    script.forEach((step, i) => {
      const delay = reduced.matches ? 0 : 260 + i * 420;
      timers.push(
        setTimeout(() => {
          const row = document.createElement("div");
          row.className = "sampler__row";
          row.dataset.verdict = step.verdict;
          row.innerHTML =
            `<span>${String(i).padStart(2, "0")}</span>` +
            `<span class="sampler__hex">${step.hex}</span>` +
            `<span class="sampler__v">${step.why}</span>`;
          list.append(row);
        }, delay),
      );
    });
  }

  list.replaceChildren(header());
  onFirstActive(root, run);
  replay?.addEventListener("click", run);
}

/* =========================================================================
   15 · signature bundle vs one C
   ========================================================================= */

function initBundle(root) {
  const tHost = root.querySelector("[data-bundle-t]");
  const sigs = root.querySelector("[data-bundle-sigs]");
  const size = root.querySelector("[data-bundle-size]");
  const cap = root.querySelector("[data-bundle-cap]");
  if (!tHost || !sigs) return;

  const OPTIONS = [1, 2, 3, 5, 7];
  const buttons = OPTIONS.map((t) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "mbtn";
    b.textContent = `${t}`;
    b.setAttribute("aria-pressed", String(t === 3));
    b.addEventListener("click", () => set(t));
    tHost.append(b);
    return b;
  });

  const CAPS = {
    1: "t = 1 is not a federation. One member, one signature, one trusted operator.",
    2: "Two signatures already means the wallet, the receiver and every later hop must know the roster and the policy.",
    3: "3-of-5 is annoying: three secp checks plus a policy check on every receive, swap, melt and restore.",
    5: "Token size and verify cost grow linearly with t. The keyset ID has to commit to a set of keys, so rotation becomes roster history.",
    7: "7-of-10 is ugly. And NUT-12 multiplies: one DLEQ per signer, or you give up offline proof that each member used its advertised key.",
  };

  function set(t) {
    buttons.forEach((b, i) => b.setAttribute("aria-pressed", String(OPTIONS[i] === t)));
    sigs.replaceChildren();
    for (let i = 0; i < t; i++) {
      const row = document.createElement("div");
      row.className = "token__sig";
      row.style.setProperty("--i", String(i));
      row.innerHTML = `<span>C_${i + 1}</span><s></s>`;
      sigs.append(row);
    }
    if (size) size.textContent = `${t} signature${t === 1 ? "" : "s"}`;
    if (cap) cap.textContent = CAPS[t];
  }

  set(3);
}

/* =========================================================================
   17 · Shamir: one share tells you nothing
   ========================================================================= */

function initShamir(root) {
  const host = root.querySelector("[data-shamir-plot]");
  const memberHost = root.querySelector("[data-shamir-members]");
  const verdict = root.querySelector("[data-shamir-verdict]");
  const cap = root.querySelector("[data-shamir-cap]");
  if (!host) return;

  // f(x) = 7 + 3x, threshold 2, members 1..3 — the example from the notes.
  const A0 = 7;
  const A1 = 3;
  const T = 2;
  const IDS = [1, 2, 3];
  const f = (x) => A0 + A1 * x;

  const X0 = 46,
    X1 = 452,
    YB = 236,
    YT = 22;
  const XMAX = 3.7,
    YMAX = 19;
  const sx = (x) => X0 + (x * (X1 - X0)) / XMAX;
  const sy = (y) => YB - (y * (YB - YT)) / YMAX;

  const gridlines = [];
  for (let x = 0; x <= 3; x++) {
    gridlines.push(
      `<line class="gridline" x1="${sx(x)}" y1="${YT}" x2="${sx(x)}" y2="${YB}"/>`,
      `<text class="t-num" x="${sx(x) - 3}" y="${YB + 14}">${x}</text>`,
    );
  }
  for (let y = 0; y <= 18; y += 3) {
    gridlines.push(
      `<line class="gridline" x1="${X0}" y1="${sy(y)}" x2="${X1}" y2="${sy(y)}"/>`,
      `<text class="t-num" x="${X0 - 22}" y="${sy(y) + 3}">${y}</text>`,
    );
  }

  // candidate lines through a single share — every intercept is still possible
  const SLOPES = [-3, 0, 6, 10];
  const candidates = IDS.flatMap((id) =>
    SLOPES.map((m) => {
      const b = f(id) - m * id;
      const y1 = b;
      const y2 = b + m * XMAX;
      return `<line class="cand" data-id="${id}" x1="${sx(0)}" y1="${sy(y1)}" x2="${sx(XMAX)}" y2="${sy(y2)}"/>
              <circle class="cand-dot" data-id="${id}" cx="${sx(0)}" cy="${sy(y1)}" r="3"/>`;
    }),
  );

  const shares = IDS.map(
    (id) => `
      <g class="share" data-id="${id}">
        <line class="share-drop" x1="${sx(id)}" y1="${sy(f(id))}" x2="${sx(id)}" y2="${YB}"/>
        <circle class="share-dot" cx="${sx(id)}" cy="${sy(f(id))}" r="5.5"/>
        <text class="t-num share-lbl" x="${sx(id) + 10}" y="${sy(f(id)) + 19}">k_${id} = ${f(id)}</text>
      </g>`,
  );

  const inner = `
    <defs>
      <clipPath id="shamir-clip">
        <rect x="${X0}" y="${YT}" width="${X1 - X0}" height="${YB - YT}"/>
      </clipPath>
    </defs>
    <g>${gridlines.join("")}</g>
    <line class="ax" x1="${X0}" y1="${YT}" x2="${X0}" y2="${YB}"/>
    <line class="ax" x1="${X0}" y1="${YB}" x2="${X1}" y2="${YB}"/>
    <text class="t-lbl" text-anchor="end" x="${X1}" y="${YB + 15}">x = signer id</text>
    <text class="t-lbl" x="${X0 - 26}" y="${YT - 8}">f(x)</text>
    <g clip-path="url(#shamir-clip)">${candidates.join("")}</g>
    <line class="truth" clip-path="url(#shamir-clip)"
      x1="${sx(0)}" y1="${sy(A0)}" x2="${sx(XMAX)}" y2="${sy(f(XMAX))}" pathLength="100"/>
    <g class="secret">
      <circle class="secret-dot" cx="${sx(0)}" cy="${sy(A0)}" r="6"/>
      <text class="t-big secret-lbl" x="${sx(0) + 14}" y="${sy(A0) + 22}">f(0) = k = ${A0}</text>
    </g>
    ${shares.join("")}`;

  const plot = makeSvg(
    "0 0 470 258",
    inner,
    "Shamir polynomial f(x) = 7 + 3x with member shares at x = 1, 2, 3. One share leaves every intercept possible; two shares fix the secret at 7.",
  );
  plot.classList.add("shamir");
  host.replaceChildren(plot);

  const selected = new Set([1, 2]);

  const buttons = IDS.map((id) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "mbtn";
    b.textContent = `M${id}`;
    b.addEventListener("click", () => {
      selected.has(id) ? selected.delete(id) : selected.add(id);
      render();
    });
    memberHost?.append(b);
    return b;
  });

  function lambdas(set) {
    const S = [...set];
    return S.map((i) => {
      let num = 1,
        den = 1;
      for (const j of S) {
        if (j === i) continue;
        num *= -j;
        den *= i - j;
      }
      return num / den;
    });
  }

  function fmt(n) {
    const t = Number.isInteger(n) ? String(n) : n.toFixed(1);
    return t.replace("-", "\u2212");
  }

  function render() {
    const S = [...selected].sort((a, b) => a - b);
    buttons.forEach((b, i) => b.setAttribute("aria-pressed", String(selected.has(IDS[i]))));

    plot.dataset.mode = S.length === 0 ? "none" : S.length < T ? "one" : "enough";
    plot.dataset.only = S.length === 1 ? String(S[0]) : "";

    for (const g of plot.querySelectorAll(".share")) {
      g.toggleAttribute("data-on", selected.has(Number(g.dataset.id)));
    }

    if (S.length === 0) {
      verdict.removeAttribute("data-ok");
      verdict.removeAttribute("data-no");
      verdict.textContent = "no shares";
      cap.textContent =
        "Pick signers. Each holds only k_i = f(i) · nothing else. The polynomial itself is never transmitted.";
      return;
    }

    if (S.length < T) {
      verdict.removeAttribute("data-ok");
      verdict.setAttribute("data-no", "");
      verdict.textContent = `${S.length} share · below t = ${T}`;
      cap.textContent = `M${S[0]} holds ${f(S[0])}·B. Every faint line through that point is a different polynomial with a different secret — so every marked intercept on the y-axis is still possible. One share learns nothing useful about k.`;
      return;
    }

    const L = lambdas(S);
    const terms = S.map((id, i) => `${fmt(L[i])}·(${f(id)}B)`)
      .join(" + ")
      .replace(/\+ \u2212/g, "\u2212 ");
    verdict.removeAttribute("data-no");
    verdict.setAttribute("data-ok", "");
    verdict.textContent = `${S.length} shares · C = ${A0}B`;
    cap.textContent = `S = {${S.join(", ")}} → λ = ${L.map(fmt).join(", ")}. C = ${terms} = ${A0}B = k·B. Any valid subset of size ≥ t lands on the same C; order does not matter, duplicates and short sets are rejected.`;
  }

  render();
}

/* =========================================================================
   18 · threshold-signing mix-and-match
   ========================================================================= */

function initMixMatch(root) {
  const host = root.querySelector("[data-mx]");
  const stepBtn = root.querySelector("[data-mx-step]");
  const resetBtn = root.querySelector("[data-mx-reset]");
  const verdict = root.querySelector("[data-mx-verdict]");
  const cap = root.querySelector("[data-mx-cap]");
  if (!host) return;

  const OUTPUTS = ["A", "B", "C", "D"];
  const MEMBERS = [1, 2, 3, 4, 5];
  const T = 3;
  const PAID = 3;

  // Milestone 12: four overlapping 3-output windows, each locally balanced.
  const WINDOWS = {
    1: ["A", "B", "C"],
    2: ["B", "C", "D"],
    3: ["C", "D", "A"],
    4: ["D", "A", "B"],
    5: ["A", "B", "C"],
  };

  const grid = document.createElement("div");
  grid.className = "mx__grid";
  grid.style.gridTemplateColumns = `3.2rem repeat(${OUTPUTS.length}, minmax(0, 1fr)) 6.5rem`;

  const cells = new Map();

  function cellKey(m, o) {
    return `${m}:${o}`;
  }

  grid.append(el("div", "mx__h", ""));
  for (const o of OUTPUTS) grid.append(el("div", "mx__h", o));
  grid.append(el("div", "mx__h", "window shown"));

  for (const m of MEMBERS) {
    grid.append(el("div", "mx__rowh", `M${m}`));
    for (const o of OUTPUTS) {
      const c = el("div", "mx__cell", "");
      c.innerHTML = "<i></i>";
      cells.set(cellKey(m, o), c);
      grid.append(c);
    }
    const w = el("div", "mx__rowh", "—");
    w.dataset.win = String(m);
    grid.append(w);
  }

  grid.append(el("div", "mx__tally", ""));
  const tallies = new Map();
  for (const o of OUTPUTS) {
    const t = el("div", "mx__tally", "0");
    tallies.set(o, t);
    grid.append(t);
  }
  grid.append(el("div", "mx__tally", `t = ${T}`));

  const note = el("p", "mx__note", "");
  host.replaceChildren(grid, note);

  let step = 0;

  function el(tag, cls, text) {
    const n = document.createElement(tag);
    n.className = cls;
    n.textContent = text;
    return n;
  }

  function render() {
    const counts = Object.fromEntries(OUTPUTS.map((o) => [o, 0]));

    for (const m of MEMBERS) {
      const shown = m <= step;
      const win = WINDOWS[m];
      for (const o of OUTPUTS) {
        const c = cells.get(cellKey(m, o));
        const signed = shown && win.includes(o);
        c.toggleAttribute("data-signed", signed);
        c.toggleAttribute("data-window", m === step && win.includes(o));
        if (signed) counts[o] += 1;
      }
      grid.querySelector(`[data-win="${m}"]`).textContent = shown ? win.join(" ") : "—";
    }

    let minted = 0;
    for (const o of OUTPUTS) {
      const t = tallies.get(o);
      t.textContent = String(counts[o]);
      const enough = counts[o] >= T;
      if (enough) minted += 1;
      if (!enough) t.removeAttribute("data-enough");
      else t.setAttribute("data-enough", minted <= PAID ? "authorized" : "true");
    }

    verdict.textContent = `paid ${PAID} · minted ${minted}`;
    verdict.toggleAttribute("data-no", minted > PAID);
    verdict.toggleAttribute("data-ok", minted > 0 && minted <= PAID);

    stepBtn.disabled = step >= MEMBERS.length;

    if (step === 0) {
      note.textContent = "";
      cap.textContent = `3-of-5. The quote paid for ${PAID} notes. Each member only ever sees a balanced 3-output request.`;
    } else if (minted > PAID) {
      note.innerHTML = `Every column now has ${T} or more shares. <b>Paid ${PAID}, minted ${minted}.</b> The unpaid note is spendable.`;
      cap.textContent =
        "Each request looked balanced locally. Across the roster, every output appeared in enough windows to gather t shares. Nobody signed anything invalid — they just signed different things.";
    } else {
      note.textContent = `M${step} was shown ${WINDOWS[step].join(" ")}. Locally that is a well-formed ${PAID}-output request.`;
      cap.textContent =
        "Keep sliding. A BLS share is independently valid: C_i = k_i · B checks against M_i's published key even if nobody else saw that B.";
    }
  }

  stepBtn?.addEventListener("click", () => {
    if (step < MEMBERS.length) step += 1;
    render();
  });
  resetBtn?.addEventListener("click", () => {
    step = 0;
    render();
  });

  render();

  onFirstActive(root, () => {
    if (reduced.matches) {
      step = MEMBERS.length;
      return render();
    }
    const tick = () => {
      if (step >= MEMBERS.length) return;
      step += 1;
      render();
      setTimeout(tick, 780);
    };
    setTimeout(tick, 600);
  });
}

/* =========================================================================
   19 · consensus picks one winner
   ========================================================================= */

function initConsensus(root) {
  const ab = root.querySelector('[data-env="ab"]');
  const bc = root.querySelector('[data-env="bc"]');
  const play = root.querySelector("[data-consensus-play]");
  const reset = root.querySelector("[data-consensus-reset]");
  const verdict = root.querySelector("[data-consensus-verdict]");
  if (!ab || !bc) return;

  let timers = [];

  function setState(env, state, label) {
    env.dataset.state = state;
    env.querySelector(".env__state").textContent = label;
  }

  function clear() {
    for (const t of timers) clearTimeout(t);
    timers = [];
  }

  function doReset() {
    clear();
    ab.removeAttribute("data-state");
    bc.removeAttribute("data-state");
    ab.querySelector(".env__state").textContent = "not submitted";
    bc.querySelector(".env__state").textContent = "not submitted";
    verdict.textContent = "idle";
    verdict.removeAttribute("data-ok");
    verdict.removeAttribute("data-no");
  }

  function run() {
    doReset();
    const d = reduced.matches ? 0 : 1;
    timers.push(
      setTimeout(() => {
        setState(ab, "pending", "submitted");
        setState(bc, "pending", "submitted");
        verdict.textContent = "ordering";
      }, 120 * d),
    );
    timers.push(
      setTimeout(() => {
        setState(ab, "accepted", "accepted · shares for A, B");
        verdict.textContent = "S_AB accepted";
        verdict.setAttribute("data-ok", "");
      }, 900 * d),
    );
    timers.push(
      setTimeout(() => {
        setState(bc, "spent", "TokenAlreadySpent · no shares written");
        verdict.textContent = "S_BC dead · C never signed";
        verdict.removeAttribute("data-ok");
        verdict.setAttribute("data-no", "");
      }, 1700 * d),
    );
  }

  play?.addEventListener("click", run);
  reset?.addEventListener("click", doReset);
  doReset();
  onFirstActive(root, run);
}

/* =========================================================================
   20 · two planes, two transports
   ========================================================================= */

function initPlanes(root) {
  const host = root.querySelector("[data-planes-plot]");
  const cap = root.querySelector("[data-planes-cap]");
  if (!host) return;

  const CX = 432,
    CY = 146,
    R = 104;
  const N = 5;
  const pos = [];
  for (let i = 0; i < N; i++) {
    const a = -Math.PI / 2 + (i * 2 * Math.PI) / N;
    pos.push({ x: CX + R * Math.cos(a), y: CY + R * Math.sin(a) });
  }

  const WX = 92,
    WY = 146;

  /* A packet that travels a straight diagonal: rotate the local frame to the
     edge, then let CSS translate along X inside it. Keeps the motion on
     transform only, so it stays on the compositor. */
  function packet(from, to, cls, delay, dur = 2.9) {
    const dx = to.x - from.x;
    const dy = to.y - from.y;
    const len = Math.hypot(dx, dy);
    const deg = (Math.atan2(dy, dx) * 180) / Math.PI;
    return `<g transform="translate(${from.x.toFixed(1)} ${from.y.toFixed(1)}) rotate(${deg.toFixed(2)})">
      <circle class="run ${cls}" cx="0" cy="0" r="3"
        style="--dx:${len.toFixed(1)}px;--dur:${dur}s;--delay:${delay.toFixed(2)}s"/>
    </g>`;
  }

  // public plane: wallet ↔ each member, straight spokes — it is a star
  const publicEdges = pos
    .map((p, i) => {
      const dead = i === 3;
      const cls = dead ? "wire wire--bad wire--ghost" : "wire wire--wallet";
      const d = Math.hypot(p.x - WX, p.y - WY);
      const ux = (p.x - WX) / d;
      const uy = (p.y - WY) / d;
      const from = { x: WX + ux * 40, y: WY + uy * 40 };
      const to = { x: p.x - ux * 18, y: p.y - uy * 18 };
      const line = `<line class="${cls}" x1="${from.x}" y1="${from.y}" x2="${to.x}" y2="${to.y}"/>`;
      return dead ? line : line + packet(to, from, "fill-g1", i * 0.18);
    })
    .join("");

  // private plane: full mesh among members
  let meshEdges = "";
  for (let i = 0; i < N; i++) {
    for (let j = i + 1; j < N; j++) {
      meshEdges += `<line class="wire wire--peer" x1="${pos[i].x}" y1="${pos[i].y}" x2="${pos[j].x}" y2="${pos[j].y}"/>`;
    }
  }

  const members = pos
    .map((p, i) => {
      const dead = i === 3;
      const below = p.y > CY - 10;
      return `<g>
        <circle class="node ${dead ? "node--bad" : "node--g1"}" cx="${p.x}" cy="${p.y}" r="16"/>
        <text class="t-big" text-anchor="middle" x="${p.x}" y="${p.y + 4}">M${i + 1}</text>
        <text class="t-num ${dead ? "fill-bad" : ""}" text-anchor="middle" x="${p.x}" y="${p.y + (below ? 33 : -24)}">${dead ? "timeout / offline" : "share " + (i + 1)}</text>
      </g>`;
    })
    .join("");

  const inner = `
    <g class="layer layer--public" data-layer="public">
      ${publicEdges}
      <text class="t-lbl fill-wallet" x="16" y="26">public plane · star · wallet is the hub</text>
      <text class="t-num" x="16" y="40" data-locator>https://mint-a.example</text>
      <rect class="node node--wallet" x="${WX - 40}" y="${WY - 22}" width="80" height="44" rx="3"/>
      <text class="t-big" text-anchor="middle" x="${WX}" y="${WY - 2}">wallet</text>
      <text class="t-num" text-anchor="middle" x="${WX}" y="${WY + 13}">aggregator</text>
      <text class="t-num fill-g1" x="16" y="${WY + 62}">keeps any 3 valid shares → one proof</text>
      <text class="t-num" x="16" y="${WY + 76}">checks each against K_i,</text>
      <text class="t-num" x="16" y="${WY + 90}">then Lagrange-interpolates</text>
    </g>
    <g class="layer layer--private" data-layer="private">
      ${meshEdges}
      <text class="t-lbl fill-peer" x="248" y="292">private plane · mesh · /federation/v1</text>
      <text class="t-num" x="248" y="306" data-locator2>consensus + DKG, member-authenticated</text>
    </g>
    ${members}`;

  const plot = makeSvg(
    "0 0 620 316",
    inner,
    "The wallet fans one Cashu request out to five members and aggregates a threshold of BLS shares; separately the five members form a private mesh for consensus and DKG.",
  );
  host.replaceChildren(plot);

  const state = { public: true, private: true, transport: "http" };

  function render() {
    for (const g of plot.querySelectorAll("[data-layer]")) {
      g.toggleAttribute("data-off", !state[g.dataset.layer]);
    }
    const loc = plot.querySelector("[data-locator]");
    if (loc) {
      loc.textContent =
        state.transport === "iroh" ? "iroh://<endpoint-id>" : "https://mint-a.example";
    }
    const loc2 = plot.querySelector("[data-locator2]");
    if (loc2) {
      loc2.textContent =
        state.transport === "iroh"
          ? "iroh://<endpoint-id> · same Axum router, Iroh is the pipe"
          : "https://mint-a.example/federation/v1 · member-authenticated";
    }

    if (!state.public && !state.private) {
      cap.textContent = "Turn a plane back on. Neither one works without the other.";
    } else if (state.public && !state.private) {
      cap.textContent =
        "Fan-out alone cannot stop mix-and-match. If M1 and M4 only ever hear from the wallet, they can still be shown different windows.";
    } else if (!state.public && state.private) {
      cap.textContent =
        "Consensus, catch-up and the DKG ceremony live here. Wallets should not be on this plane at all.";
    } else if (state.transport === "iroh") {
      cap.textContent =
        "Same two planes, different pipe. Tests already dial a roster where some members are HTTP and some are Iroh from one transport.";
    } else {
      cap.textContent =
        "The wallet is the hub and the aggregator. Members never send shares to each other; it does not wait for everyone, and a slow minority is simply ignored.";
    }
  }

  for (const btn of root.querySelectorAll("[data-plane]")) {
    btn.addEventListener("click", () => {
      const key = btn.dataset.plane;
      state[key] = !state[key];
      btn.setAttribute("aria-pressed", String(state[key]));
      render();
    });
  }

  for (const btn of root.querySelectorAll("[data-transport]")) {
    btn.addEventListener("click", () => {
      state.transport = btn.dataset.transport;
      for (const b of root.querySelectorAll("[data-transport]")) {
        b.setAttribute("aria-pressed", String(b.dataset.transport === state.transport));
      }
      render();
    });
  }

  render();
}

/* =========================================================================
   21 · dealer-free BLS DKG
   ========================================================================= */

function initDkg(root) {
  const host = root.querySelector("[data-dkg-plot]");
  const phaseHost = root.querySelector("[data-dkg-phases]");
  const verdict = root.querySelector("[data-dkg-verdict]");
  const cap = root.querySelector("[data-dkg-cap]");
  if (!host) return;

  const N = 4;
  const CX = 134,
    CY = 134,
    R = 72;
  const pos = [];
  for (let i = 0; i < N; i++) {
    const a = -Math.PI / 4 + (i * 2 * Math.PI) / N;
    pos.push({
      x: CX + R * Math.cos(a),
      y: CY + R * Math.sin(a),
      ux: Math.cos(a),
      uy: Math.sin(a),
    });
  }

  const PHASES = [
    {
      key: "agree",
      label: "agree",
      verdict: "roster · t · unit",
      cap: "Agree what is being generated: roster, threshold, unit, amounts, fees, expiry. If those differ, the ceremony cannot finish.",
    },
    {
      key: "commit",
      label: "commit",
      verdict: "hashes published",
      cap: "Hash-commit first. Every member publishes H(my later reveal) before anyone shows actual G2 points — that stops the last member from peeking at everyone else's keys and then choosing a convenient polynomial.",
    },
    {
      key: "reveal",
      label: "reveal",
      verdict: "commitments open",
      cap: "Open the G2 commitments. The first coefficient is that member's public piece of K; the rest let others check the evaluations that follow. A reveal that does not match the earlier hash is rejected.",
    },
    {
      key: "deliver",
      label: "deliver",
      verdict: "f_j(i) delivered",
      cap: "Member j sends member i the value f_j(i) over the authenticated private plane. Member i checks f_j(i)·G2 == evaluate j's public polynomial at i. A lie is visible; a good share is kept. Raw coefficients were thrown away at generation time.",
    },
    {
      key: "sum",
      label: "sum",
      verdict: "k_i = Σ f_j(i)",
      cap: "Member i adds every verified contribution and gets its final share. Publicly, everyone adds the constant-term commitments and gets the aggregate mint key K — plus each K_i. That is the v3 keyset, plus federation metadata.",
    },
    {
      key: "activate",
      label: "activate",
      verdict: "K live",
      cap: "Hash the finished public ceremony, collect member signatures, and only then activate the keyset. The secret k never existed in one place, and the wallet never sees the dance — it sees one mint public key.",
    },
  ];

  /* Per-member decorations sit radially outward, so nothing lands on a chord. */
  const nodes = pos
    .map((p, i) => {
      const d = { anchor: p.ux > 0 ? "start" : "end" };
      const bx = p.x + p.ux * 26;
      const by = p.y + p.uy * 26;
      const px = -p.uy;
      const py = p.ux;
      const dots = [-1, 0, 1]
        .map(
          (k) =>
            `<circle class="c-dot fill-g2" cx="${(bx + px * k * 9).toFixed(1)}" cy="${(by + py * k * 9).toFixed(1)}" r="3"/>`,
        )
        .join("");
      return `
    <g>
      <g class="layer" data-min="1">
        <circle class="ring" cx="${p.x}" cy="${p.y}" r="25"/>
      </g>
      <g class="layer" data-min="2">${dots}</g>
      <circle class="node node--g2" cx="${p.x}" cy="${p.y}" r="18"/>
      <text class="t-big" text-anchor="middle" x="${p.x}" y="${p.y + 4}">M${i + 1}</text>
      <g class="layer" data-min="4">
        <text class="t-num fill-good" text-anchor="${d.anchor}" x="${(p.x + p.ux * 46).toFixed(1)}" y="${(p.y + p.uy * 46 + 3).toFixed(1)}">k_${i + 1} ✓</text>
      </g>
    </g>`;
    })
    .join("");

  let edges = "";
  let k = 0;
  for (let i = 0; i < N; i++) {
    for (let j = 0; j < N; j++) {
      if (i === j) continue;
      const a = pos[j],
        b = pos[i];
      const dx = (b.x - a.x) * 0.7;
      const dy = (b.y - a.y) * 0.7;
      const len = Math.hypot(dx, dy);
      const deg = (Math.atan2(dy, dx) * 180) / Math.PI;
      edges += `<g class="layer" data-min="3">
        <line class="wire wire--peer" x1="${a.x}" y1="${a.y}" x2="${(a.x + dx).toFixed(1)}" y2="${(a.y + dy).toFixed(1)}"/>
        <g transform="translate(${a.x.toFixed(1)} ${a.y.toFixed(1)}) rotate(${deg.toFixed(2)})">
          <circle class="run fill-peer" cx="0" cy="0" r="2.6"
            style="--dx:${len.toFixed(1)}px;--dur:2.4s;--delay:${(k * 0.12).toFixed(2)}s"/>
        </g>
      </g>`;
      k += 1;
    }
  }

  const RX = 288;
  const inner = `
    <rect class="enclosure" x="${CX - R - 42}" y="${CY - R - 42}" width="${2 * (R + 42)}" height="${2 * (R + 42)}" rx="4"/>
    <text class="t-lbl fill-peer" text-anchor="middle" x="${CX}" y="${CY + R + 32}">private member plane</text>
    ${edges}
    ${nodes}
    <line class="wire wire--ghost" x1="${RX - 24}" y1="12" x2="${RX - 24}" y2="248"/>
    <g class="layer" data-min="4">
      <rect class="node node--g2" x="${RX}" y="16" width="196" height="46" rx="3"/>
      <text class="t-gate t-gate--g2" x="${RX + 12}" y="37">K = ΣC_j[0]</text>
      <text class="t-num" x="${RX + 12}" y="52">= k · G2 · aggregate mint key</text>
    </g>
    <g class="layer" data-min="5">
      <rect class="node node--good" x="${RX}" y="74" width="196" height="46" rx="3"/>
      <text class="t-big fill-good" x="${RX + 12}" y="95">transcript signed</text>
      <text class="t-num" x="${RX + 12}" y="110">v3 keyset activated</text>
    </g>
    <text class="t-lbl" x="${RX}" y="156">wallet sees</text>
    <text class="t-num" x="${RX}" y="170">one G2 key per amount</text>
    <text class="t-num fill-g2" x="${RX}" y="184">02ce4c47…3c5d</text>
    <text class="t-lbl" x="${RX}" y="216">nobody ever holds</text>
    <text class="t-num fill-bad" x="${RX}" y="230">k = f_1(0) + … + f_n(0)</text>`;

  const plot = makeSvg(
    "0 0 494 258",
    inner,
    "Four members commit, reveal G2 polynomial commitments, privately deliver evaluations to each other, sum them into personal shares, and activate one aggregate mint key.",
  );
  plot.classList.add("dkg");
  host.replaceChildren(plot);

  let phase = 0;

  const buttons = PHASES.map((p, i) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "phase";
    b.textContent = p.label;
    b.addEventListener("click", () => {
      phase = i;
      render();
    });
    phaseHost?.append(b);
    return b;
  });

  function render() {
    buttons.forEach((b, i) => {
      b.toggleAttribute("data-current", i === phase);
      b.toggleAttribute("data-done", i < phase);
    });
    for (const g of plot.querySelectorAll(".layer")) {
      g.toggleAttribute("data-on", phase >= Number(g.dataset.min));
    }
    verdict.textContent = PHASES[phase].verdict;
    verdict.toggleAttribute("data-ok", phase === PHASES.length - 1);
    cap.textContent = PHASES[phase].cap;
  }

  render();
  onFirstActive(root, () => {
    if (reduced.matches) return;
    let i = 0;
    const tick = () => {
      if (i >= 3) return;
      i += 1;
      phase = i;
      render();
      setTimeout(tick, 900);
    };
    setTimeout(tick, 700);
  });
}

/* =========================================================================
   28 · the roster is the constitution
   ========================================================================= */

function initRoster(root) {
  const rows = [...root.querySelectorAll(".tr")];
  const idOut = root.querySelector("[data-roster-id] code");
  const digestOut = root.querySelector("[data-roster-digest] code");
  const idBox = root.querySelector("[data-roster-id]");
  const digestBox = root.querySelector("[data-roster-digest]");
  const verdict = root.querySelector("[data-roster-verdict]");
  const cap = root.querySelector("[data-roster-cap]");
  if (!rows.length) return;

  const GENESIS = { id: "a91c7fe0…4d2b", digest: "3e08bb14…c7a6" };

  const ACTS = {
    reset: {
      fields: [],
      id: GENESIS.id,
      digest: GENESIS.digest,
      verdict: "genesis",
      state: null,
      cap: "Wallets check config_digest in every member's mint-info advertisement — so even fields that are not in the ID still have to match across the roster, or wallets refuse to aggregate.",
    },
    tls: {
      fields: ["tls"],
      id: GENESIS.id,
      digest: GENESIS.digest,
      verdict: "same federation",
      state: "ok",
      cap: "TLS certificates are not in either hash. You can renew Let's Encrypt for the same name forever. You cannot change the name.",
    },
    keyset: {
      fields: ["keysets"],
      id: GENESIS.id,
      digest: "b7d4102a…91ef",
      verdict: "same id · new digest",
      state: "ok",
      cap: "KeysetRotation is the only consensus mutation of the public signing surface. The digest moves, the federation_id does not, and wallets re-validate the advertisement. Operators need that distinction burned into their heads.",
    },
    dns: {
      fields: ["dns"],
      id: "0f5b2c88…7e13",
      digest: "cc1a4f60…20d9",
      verdict: "new federation",
      state: "no",
      cap: "Both URLs are inside federation_id. Update nginx and the other members still dial the old host; edit one config and that member's derived ID no longer matches; tell wallets the new URL and the mint-info digest no longer matches the imported federation. There is no rotation for locators — the honest path is a new setup: new proposal, new authorization, new ID, new dual DKG, migrate users.",
    },
  };

  function apply(name) {
    const act = ACTS[name] ?? ACTS.reset;
    for (const row of rows) {
      row.toggleAttribute("data-changed", act.fields.includes(row.dataset.field));
    }
    idOut.textContent = act.id;
    digestOut.textContent = act.digest;
    idBox.toggleAttribute("data-changed", act.id !== GENESIS.id);
    digestBox.toggleAttribute("data-changed", act.digest !== GENESIS.digest);
    verdict.textContent = act.verdict;
    verdict.toggleAttribute("data-ok", act.state === "ok");
    verdict.toggleAttribute("data-no", act.state === "no");
    cap.textContent = act.cap;
    for (const b of root.querySelectorAll("[data-roster-act]")) {
      b.toggleAttribute("data-on", b.dataset.rosterAct === name && name !== "reset");
    }
  }

  for (const b of root.querySelectorAll("[data-roster-act]")) {
    b.addEventListener("click", () => apply(b.dataset.rosterAct));
  }

  apply("reset");
}

/* =========================================================================
   wiring
   ========================================================================= */

const INIT = {
  sampler: initSampler,
  bundle: initBundle,
  shamir: initShamir,
  mixmatch: initMixMatch,
  consensus: initConsensus,
  planes: initPlanes,
  dkg: initDkg,
  roster: initRoster,
};

for (const [name, fn] of Object.entries(INIT)) {
  for (const el of document.querySelectorAll(`[data-fig="${name}"]`)) {
    try {
      fn(el);
    } catch (err) {
      console.error(`figure "${name}" failed`, err);
    }
  }
}
