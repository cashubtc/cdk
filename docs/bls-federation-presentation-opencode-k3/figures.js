/* Figure system: staged SVG/HTML diagrams for the federation deck.
   Each figure exposes { labels, go(i) }. The framework adds step pills,
   replay buttons, autoplay, and packet animation. */
(function () {
  "use strict";

  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const defs = new Map();
  const apis = new Map();
  const tokens = new Map();
  const stepListeners = [];

  /* ── tiny DOM helpers ─────────────────────────────────────── */
  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  function prepDraw(root) {
    root.querySelectorAll(".fx-draw").forEach((p) => {
      try {
        const len = p.getTotalLength();
        p.style.setProperty("--len", len);
      } catch (_) { /* non-path */ }
    });
  }

  function applyStages(root, i) {
    root.querySelectorAll("[data-stage]").forEach((elm) => {
      elm.classList.toggle("in", +elm.dataset.stage <= i);
    });
  }

  /* dot that travels along a path once */
  function runPacket(svg, path, color, delay, dur, name, token) {
    let dot = path._dot;
    if (!dot) {
      dot = document.createElementNS("http://www.w3.org/2000/svg", "circle");
      dot.setAttribute("r", "4");
      dot.classList.add("pkt");
      svg.appendChild(dot);
      path._dot = dot;
    }
    dot.setAttribute("fill", color);
    dot.style.opacity = "0";
    const len = path.getTotalLength();
    const t0 = performance.now() + (reduceMotion ? 0 : delay);
    const D = reduceMotion ? 1 : dur;
    function frame(now) {
      if (tokens.get(name) !== token) return;
      const t = Math.min(1, Math.max(0, (now - t0) / D));
      if (t > 0) dot.style.opacity = "1";
      const e = 1 - Math.pow(1 - t, 3); // ease-out cubic
      const pt = path.getPointAtLength(len * e);
      dot.setAttribute("cx", pt.x);
      dot.setAttribute("cy", pt.y);
      if (t < 1) requestAnimationFrame(frame);
      else dot.style.opacity = "0.9";
    }
    requestAnimationFrame(frame);
  }

  function runPackets(root, svg, i, name, token) {
    root.querySelectorAll("path[data-pktstage]").forEach((p) => {
      const s = +p.dataset.pktstage;
      if (s === i) {
        runPacket(svg, p, p.dataset.pkt, +(p.dataset.delay || 0), +(p.dataset.dur || 750), name, token);
      } else if (s > i && p._dot) {
        p._dot.style.opacity = "0";
      }
    });
  }

  /* ── framework ────────────────────────────────────────────── */
  function define(name, factory) { defs.set(name, factory); }

  function buildPills(name, api) {
    const host = document.querySelector(`[data-steps-for="${name}"]`);
    if (!host || !api.labels.length) return;
    api.labels.forEach((label, i) => {
      const b = el("button", "step-pill", label);
      b.addEventListener("click", () => { api._manual = true; go(name, i); });
      host.appendChild(b);
    });
  }

  function paintPills(name, idx) {
    const host = document.querySelector(`[data-steps-for="${name}"]`);
    if (!host) return;
    Array.from(host.children).forEach((b, i) => {
      b.classList.toggle("active", i === idx);
      b.classList.toggle("done", i < idx);
    });
  }

  function go(name, i) {
    const api = apis.get(name);
    if (!api) return 0;
    const idx = Math.max(0, Math.min(api.labels.length - 1, i));
    const token = (tokens.get(name) || 0) + 1;
    tokens.set(name, token);
    api.go(idx, token);
    paintPills(name, idx);
    stepListeners.forEach((cb) => cb(name, idx));
    return idx;
  }

  function play(name) {
    const api = apis.get(name);
    if (!api) return 0;
    const last = api.labels.length - 1;
    if (last < 0) return 0;
    if (reduceMotion) return go(name, last);
    api._manual = false;
    go(name, 0);
    for (let i = 1; i <= last; i++) {
      setTimeout(() => {
        if (api._manual) return; // user took over; don't fight them
        go(name, i);
      }, 420 + i * 950);
    }
    return last;
  }

  window.Figures = {
    init() {
      document.querySelectorAll(".figure-body[id]").forEach((body) => {
        const name = body.id.replace(/^fig/, "").replace(/^./, (c) => c.toLowerCase());
        const factory = defs.get(name);
        if (!factory) return;
        const api = factory(body);
        apis.set(name, api);
        buildPills(name, api);
        // start at final state for printing / no-JS-anim contexts only if reduced motion
        if (reduceMotion) api.go(api.labels.length - 1, 0);
      });
      document.querySelectorAll("[data-replay]").forEach((btn) => {
        btn.addEventListener("click", () => play(btn.dataset.replay));
      });
    },
    autoplay(name) {
      const api = apis.get(name);
      if (!api || api._played) return;
      api._played = true;
      setTimeout(() => play(name), 500);
    },
    play,
    step: go,
    stepCount(name) {
      const api = apis.get(name);
      return api ? api.labels.length : 0;
    },
    onStepChange(cb) { stepListeners.push(cb); },
  };

  /* ═══════════════════════════════════════════════════════════
     FIGURE: keyset v3 ID construction
     ═══════════════════════════════════════════════════════════ */
  define("keysetId", (body) => {
    body.innerHTML = `
    <svg viewBox="0 0 640 330" role="img" aria-label="v3 keyset ID construction">
      <g class="fx" data-stage="0">
        <rect x="30" y="18" width="580" height="34" rx="7" class="chip"/>
        <text x="46" y="39" class="t">1 · sort public keys by amount</text>
      </g>
      <g class="fx" data-stage="1">
        <rect x="30" y="66" width="580" height="34" rx="7" class="chip"/>
        <text x="46" y="87" class="t">2 · join <tspan class="t-sky">"amount:hex(G2 pubkey)"</tspan> entries</text>
      </g>
      <g class="fx" data-stage="2">
        <rect x="30" y="114" width="580" height="34" rx="7" class="chip"/>
        <text x="46" y="135" class="t">3 · append <tspan class="t-sky">"|unit:… |input_fee_ppk:… |final_expiry:…"</tspan></text>
      </g>
      <g class="fx" data-stage="3">
        <rect x="30" y="162" width="580" height="34" rx="7" class="chip"/>
        <text x="46" y="183" class="t">4 · SHA-256 → 32 bytes <tspan class="t-dim">(the v2 recipe, unchanged)</tspan></text>
      </g>
      <g class="fx" data-stage="4">
        <text x="46" y="222" class="t">5 · prefix version byte</text>
        <rect x="30" y="236" width="46" height="40" rx="6" class="chip-hot"/>
        <text x="53" y="261" class="t t-amber" text-anchor="middle" style="font-weight:700">02</text>
        <rect x="76" y="236" width="534" height="40" rx="6" class="chip"/>
        <text x="343" y="261" class="t t-dim" text-anchor="middle">32 hash bytes — over compressed G2 keys</text>
        <text x="30" y="304" class="t t-dim">= 33-byte keyset ID · same width as v2 · e.g. <tspan class="t-amber">02ce4c47…4c743c5d</tspan></text>
      </g>
      <path class="fx-draw edge" data-stage="1" d="M320 52 L320 66"/>
      <path class="fx-draw edge" data-stage="2" d="M320 100 L320 114"/>
      <path class="fx-draw edge" data-stage="3" d="M320 148 L320 162"/>
      <path class="fx-draw edge" data-stage="4" d="M320 196 L320 232"/>
    </svg>`;
    prepDraw(body);
    return {
      labels: ["sort", "join", "append", "hash", "prefix 02"],
      go(i) { applyStages(body, i); },
    };
  });

  /* ═══════════════════════════════════════════════════════════
     FIGURE: v3 proof lifecycle pipeline
     ═══════════════════════════════════════════════════════════ */
  define("lifecycle", (body) => {
    const stations = [
      { x: 40,  who: "wallet", c: "sky",   t: "derive",   eq: "secret, r ← seed", sub: "NUT-13 KDF" },
      { x: 220, who: "wallet", c: "sky",   t: "blind",    eq: "B = r · H(secret)", sub: "G1, DST hash-to-curve" },
      { x: 400, who: "mint",   c: "amber", t: "sign",     eq: "C_ = k · B", sub: "no DLEQ attached" },
      { x: 580, who: "wallet", c: "sky",   t: "verify blind", eq: "e(C_, G2) == e(B, K)", sub: "before trusting the mint" },
      { x: 760, who: "wallet", c: "sky",   t: "unblind",  eq: "C = r⁻¹ · C_", sub: "scalar inverse in Fr" },
      { x: 940, who: "wallet", c: "green", t: "verify proof", eq: "e(C, G2) ==", eq2: "e(H(secret), K)", sub: "ordinary v3 proof" },
    ];
    const W = 1100, Y = 96;
    let s = `<svg viewBox="0 0 ${W} 240" id="svgLifecycle" role="img" aria-label="v3 proof lifecycle">`;
    // zone brackets
    s += `<g class="fx" data-stage="0">
      <text x="120" y="30" class="t t-sky t-sm" text-anchor="middle">WALLET</text>
      <text x="400" y="30" class="t t-amber t-sm" text-anchor="middle">MINT</text>
      <text x="860" y="30" class="t t-sky t-sm" text-anchor="middle">WALLET</text></g>`;
    stations.forEach((st, i) => {
      const cls = st.c === "amber" ? "node-amber" : st.c === "green" ? "node-green" : "node-sky";
      s += `<g class="fx" data-stage="${i}">
        <rect x="${st.x}" y="64" width="150" height="104" rx="10" class="${cls}"/>
        <text x="${st.x + 75}" y="90" class="t t-title" text-anchor="middle">${st.t}</text>
        <text x="${st.x + 75}" y="${st.eq2 ? 110 : 116}" class="t ${st.c === "green" ? "t-green" : "t-ink"}" text-anchor="middle" style="font-size:10.5px">${st.eq}</text>
        ${st.eq2 ? `<text x="${st.x + 75}" y="124" class="t t-green" text-anchor="middle" style="font-size:10.5px">${st.eq2}</text>` : ""}
        <text x="${st.x + 75}" y="146" class="t t-dim t-sm" text-anchor="middle">${st.sub}</text>
      </g>`;
      if (i < stations.length - 1) {
        const x1 = st.x + 150, x2 = stations[i + 1].x;
        s += `<path class="fx-draw edge" data-stage="${i + 1}" d="M${x1} ${Y + 20} L${x2} ${Y + 20}" id="lcEdge${i}"/>`;
      }
    });
    // DLEQ rejected badge
    s += `<g class="fx" data-stage="2">
      <rect x="386" y="186" width="178" height="26" rx="13" class="chip-red"/>
      <text x="475" y="203" class="t t-red t-sm" text-anchor="middle">DLEQ on v3 → reject</text></g>`;
    // packet path (invisible spine below the stations)
    s += `<path d="M115 186 L1015 186" fill="none" stroke="none" id="lcSpine"/>`;
    s += `</svg>`;
    body.innerHTML = s;
    prepDraw(body);
    const svg = body.querySelector("svg");
    const spine = body.querySelector("#lcSpine");
    const len = spine.getTotalLength();
    const stops = stations.map((st) => (st.x + 75 - 115) / (1015 - 115));
    return {
      labels: stations.map((st) => st.t),
      go(i, token) {
        applyStages(body, i);
        tokens.set("lifecycle", token);
        const dot = spine._dot || (() => {
          const d = document.createElementNS("http://www.w3.org/2000/svg", "circle");
          d.setAttribute("r", "5"); d.setAttribute("fill", "#F5A524"); d.classList.add("pkt");
          svg.appendChild(d); spine._dot = d; return d;
        })();
        // glide the packet to station i
        const from = parseFloat(dot.dataset.pos || "0");
        const to = stops[i];
        const t0 = performance.now();
        const D = reduceMotion ? 1 : 600;
        (function frame(now) {
          if (tokens.get("lifecycle") !== token) return;
          const t = Math.min(1, (now - t0) / D);
          const e = 1 - Math.pow(1 - t, 3);
          const pos = from + (to - from) * e;
          const pt = spine.getPointAtLength(len * pos);
          dot.setAttribute("cx", pt.x); dot.setAttribute("cy", pt.y);
          dot.style.opacity = "1";
          if (t < 1) requestAnimationFrame(frame);
          else dot.dataset.pos = String(to);
        })(t0);
      },
    };
  });

  /* ═══════════════════════════════════════════════════════════
     FIGURE: Shamir threshold interpolation (interactive subsets)
     f(x) = 7 + 3x, k = f(0) = 7 — the notes' tiny 2-of-3 example
     ═══════════════════════════════════════════════════════════ */
  define("shamir", (body) => {
    const px = (x) => 90 + x * 130;
    const py = (y) => 340 - y * 16.5;
    const members = [
      { id: 1, k: 10 }, { id: 2, k: 13 }, { id: 3, k: 16 },
    ];
    const subsets = {
      "1,2": { lam: [{ i: 1, l: "2" }, { i: 2, l: "−1" }], expr: "2·(10B) + (−1)·(13B)" },
      "2,3": { lam: [{ i: 2, l: "3" }, { i: 3, l: "−2" }], expr: "3·(13B) + (−2)·(16B)" },
      "1,3": { lam: [{ i: 1, l: "3/2" }, { i: 3, l: "−1/2" }], expr: "1.5·(10B) + (−0.5)·(16B)" },
    };
    let current = "1,2";

    let s = `<div class="shm"><svg viewBox="0 0 620 400" id="svgShamir" role="img" aria-label="Shamir interpolation graph">`;
    // axes
    s += `<g class="fx" data-stage="0">
      <path class="axis" d="M${px(-0.5)} ${py(0)} L${px(3.5)} ${py(0)}"/>
      <path class="axis" d="M${px(0)} ${py(-0.8)} L${px(0)} ${py(18)}"/>`;
    for (let x = 0; x <= 3; x++) s += `<text x="${px(x)}" y="${py(0) + 18}" class="t t-dim t-sm" text-anchor="middle">${x === 0 ? "0 ← secret lives here" : x}</text>`;
    s += `</g>`;
    // polynomial line
    s += `<path class="fx-draw curve" data-stage="0" d="M${px(-0.3)} ${py(6.1)} L${px(3.3)} ${py(16.9)}"/>`;
    s += `<g class="fx" data-stage="0"><text x="${px(2.35)}" y="${py(13.1)}" class="t t-amber" style="font-style:italic">f(x) = 7 + 3x</text></g>`;
    // secret at x=0
    s += `<g class="fx" data-stage="0">
      <circle id="shmSecret" cx="${px(0)}" cy="${py(7)}" r="6" class="pt-secret"/>
      <text x="${px(0) - 12}" y="${py(7) - 12}" class="t t-amber t-sm" text-anchor="end">k = f(0) = 7</text></g>`;
    // member points
    members.forEach((m, idx) => {
      s += `<g class="fx-pop" data-stage="1">
        <circle cx="${px(m.id)}" cy="${py(m.k)}" r="7" class="pt-member"/>
        <text x="${px(m.id)}" y="${py(m.k) - 16}" class="t t-title" text-anchor="middle">M${m.id} · k${m.id} = ${m.k}</text>
      </g>`;
    });
    // interpolation lines group (rebuilt per subset)
    s += `<g id="shmLines"></g>`;
    // aggregate marker
    s += `<g class="fx-pop" data-stage="3" id="shmAgg">
      <circle cx="${px(0)}" cy="${py(7)}" r="10" class="pt-agg"/>
      <text x="${px(0) + 18}" y="${py(7) + 26}" class="t t-green">C = k·B — identical for every 2-of-3 subset</text>
      <text x="${px(0) + 18}" y="${py(7) + 44}" class="t t-dim t-sm">then unblind: proof = r⁻¹ · C</text>
    </g>`;
    s += `</svg>`;
    // side ledger
    s += `<div class="shm-side">
      <div class="shm-row fx" data-stage="0"><span class="shm-k">polynomial</span>f(x) = 7 + 3x &nbsp;·&nbsp; degree t−1 = 1</div>
      <div class="shm-row fx" data-stage="1"><span class="shm-k">shares</span>k₁ = 10 · k₂ = 13 · k₃ = 16</div>
      <div class="shm-row fx" data-stage="2"><span class="shm-k">subset</span>
        <span class="shm-subsets" id="shmSubsets"></span></div>
      <div class="shm-row fx" data-stage="2"><span class="shm-k">lagrange at 0</span><span id="shmLambda"></span></div>
      <div class="shm-row fx" data-stage="3"><span class="shm-k">interpolate</span><span id="shmExpr"></span></div>
      <div class="shm-row shm-note fx" data-stage="3">real system: scalars in Fr, points in G1 — same interpolation in the exponent. signer id 0 is forbidden: f(0) is the secret.</div>
    </div></div>`;
    body.innerHTML = s;
    prepDraw(body);

    const lineHost = body.querySelector("#shmLines");
    const lamEl = body.querySelector("#shmLambda");
    const exprEl = body.querySelector("#shmExpr");
    const subHost = body.querySelector("#shmSubsets");

    Object.keys(subsets).forEach((key) => {
      const b = el("button", "shm-chip", `{${key}}`);
      b.addEventListener("click", () => { current = key; renderSubset(true); });
      subHost.appendChild(b);
    });

    function renderSubset(animate) {
      const sub = subsets[current];
      lineHost.innerHTML = "";
      sub.lam.forEach(({ i }) => {
        const m = members[i - 1];
        const p = document.createElementNS("http://www.w3.org/2000/svg", "path");
        p.setAttribute("d", `M${px(m.id)} ${py(m.k)} L${px(0)} ${py(7)}`);
        p.setAttribute("class", "interp-line");
        lineHost.appendChild(p);
        if (animate && !reduceMotion) {
          const L = p.getTotalLength();
          p.style.strokeDasharray = L;
          p.style.strokeDashoffset = L;
          p.getBoundingClientRect();
          p.style.transition = "stroke-dashoffset 640ms cubic-bezier(0.77,0,0.175,1)";
          p.style.strokeDashoffset = "0";
        }
      });
      lamEl.textContent = sub.lam.map(({ i, l }) => `λ${i} = ${l}`).join("   ");
      exprEl.innerHTML = `C = ${sub.expr} = <tspan class="t-green" style="color:var(--green);font-weight:700">7B</tspan>`;
      Array.from(subHost.children).forEach((b) => b.classList.toggle("active", b.textContent === `{${current}}`));
    }
    renderSubset(false);

    return {
      labels: ["curve", "shares", "subset λ", "aggregate"],
      go(i) { applyStages(body, i); if (i >= 2) renderSubset(i === 2); },
    };
  });

  /* ═══════════════════════════════════════════════════════════
     FIGURE: sliding-window mix-and-match
     ═══════════════════════════════════════════════════════════ */
  define("window", (body) => {
    // shares: which member signs which output per stage
    // stage1: M1-3 {A,B}, M4-5 {B,C}   stage2: M3 also {B,C}
    const grid = [
      // M1..M5 × A,B,C  — dot class: w1 (amber) | w2 (sky) | bad (red)
      ["w1", "w1", ""],     // M1
      ["w1", "w1", ""],     // M2
      ["w1", "w1", "bad"],  // M3 — saw both windows
      ["", "w2", "w2"],     // M4
      ["", "w2", "w2"],     // M5
    ];
    let h = `<div class="win"><div class="win-grid" role="img" aria-label="Sliding window share matrix">
      <div class="win-row win-head"><span class="win-m"></span><span>A</span><span>B</span><span>C</span></div>`;
    grid.forEach((row, r) => {
      h += `<div class="win-row"><span class="win-m">M${r + 1}</span>`;
      row.forEach((cell, c) => {
        const stage = cell === "w1" || (cell === "w2" && c === 1) ? 1 : cell === "w2" ? 1 : 2;
        const st = cell === "bad" ? 2 : 1;
        h += `<span class="win-cell">${cell ? `<span class="win-dot ${cell}" data-stage="${st}"></span>` : ""}</span>`;
      });
      h += `</div>`;
    });
    h += `</div><div class="win-side">
      <div class="win-panel fx" data-stage="0">
        <p class="shm-k">setup</p>
        <p>paid quote = <b>2 tokens</b> · honest outputs <b>A, B</b> · attacker prepares a third: <b class="t-red">C</b></p>
      </div>
      <div class="win-panel fx" data-stage="1">
        <p class="shm-k">pass 1 — two windows</p>
        <p><span class="t-amber">M1 M2 M3 ← {A, B}</span><br><span class="t-sky">M4 M5 ← {B, C}</span></p>
        <p class="t-dim">locally each request looks balanced: two outputs for a two-token quote</p>
        <p class="win-tally">A ✓✓✓ &nbsp; B ✓✓✓✓✓ &nbsp; C ✓✓ <span class="t-dim">(only 2 — safe for now)</span></p>
      </div>
      <div class="win-panel fx" data-stage="2">
        <p class="shm-k">pass 2 — slide the window</p>
        <p><span class="t-red">M3 is also shown {B, C}</span> and signs again</p>
        <p class="win-tally">A ✓✓✓ &nbsp; B ✓✓✓✓✓ &nbsp; <span class="t-red">C ✓✓✓</span></p>
        <p class="win-banner">paid 2 · minted 3 — every column reached t = 3</p>
      </div>
      <div class="win-panel fx" data-stage="3">
        <p class="shm-k">with consensus</p>
        <p>{A,B} and {B,C} are <b>different envelopes</b>, different <code>operation_id</code>s.<br>
        The box accepts <span class="t-green">S_AB</span> first; S_BC conflicts on the same quote / spent inputs.</p>
        <p class="win-tally"><span class="t-green">A, B signed</span> · <span class="t-red">C: no shares ever exist</span></p>
        <p class="t-dim">members don't gossip about signatures they made — they agree before any signature exists</p>
      </div>
    </div></div>`;
    body.innerHTML = h;
    return {
      labels: ["setup", "two windows", "slide", "with consensus"],
      go(i) {
        body.querySelectorAll(".win-dot").forEach((d) => d.classList.toggle("in", +d.dataset.stage <= i));
        const panel = Math.min(Math.max(i, 0), 3);
        body.querySelectorAll(".win-panel").forEach((p) => p.classList.toggle("in", +p.dataset.stage === panel));
      },
    };
  });

  /* ═══════════════════════════════════════════════════════════
     FIGURE: consensus before signing
     ═══════════════════════════════════════════════════════════ */
  define("consensus", (body) => {
    let s = `<svg viewBox="0 0 1100 310" id="svgConsensus" role="img" aria-label="Consensus picks one envelope">`;
    // wallet
    s += `<g class="fx" data-stage="0">
      <rect x="30" y="125" width="110" height="56" rx="10" class="node-sky"/>
      <text x="85" y="149" class="t t-title" text-anchor="middle">wallet</text>
      <text x="85" y="167" class="t t-dim t-sm" text-anchor="middle">P1, P2</text></g>`;
    // envelopes
    s += `<g class="fx" data-stage="0">
      <rect x="220" y="52" width="230" height="44" rx="8" class="node-sky"/>
      <text x="335" y="71" class="t t-sm" text-anchor="middle">S_AB = hash(P1,P2 → A,B)</text>
      <text x="335" y="87" class="t t-dim t-sm" text-anchor="middle">the honest swap</text></g>
      <g class="fx" data-stage="0">
      <rect x="220" y="212" width="230" height="44" rx="8" class="node-red"/>
      <text x="335" y="231" class="t t-sm" text-anchor="middle">S_BC = hash(P1,P2 → B,C)</text>
      <text x="335" y="247" class="t t-dim t-sm" text-anchor="middle">same inputs, different outputs</text></g>`;
    // arrows wallet->envelopes->box
    s += `<path class="fx-draw edge-sky" data-stage="1" data-pktstage="1" id="csP1" data-pkt="#4CC2FF" data-delay="0"
        d="M140 140 C 190 130, 190 96, 220 76"/>
      <path class="fx-draw edge-red" data-stage="1" data-pktstage="1" id="csP2" data-pkt="#F4586B" data-delay="150"
        d="M140 168 C 190 178, 190 212, 220 234"/>
      <path class="fx-draw edge-sky" data-stage="1" data-pktstage="1" id="csP3" data-pkt="#4CC2FF" data-delay="350"
        d="M450 74 C 520 74, 540 110, 575 128"/>
      <path class="fx-draw edge-red" data-stage="1" data-pktstage="1" id="csP4" data-pkt="#F4586B" data-delay="500"
        d="M450 234 C 520 234, 540 196, 575 178"/>`;
    // consensus box
    s += `<g class="fx" data-stage="1">
      <rect x="575" y="95" width="220" height="120" rx="12" class="node-box"/>
      <text x="685" y="140" class="t t-title" text-anchor="middle">consensus box</text>
      <text x="685" y="160" class="t t-dim t-sm" text-anchor="middle">AlephBFT · one total order</text>
      <text x="685" y="178" class="t t-dim t-sm" text-anchor="middle">envelopes in → accepted facts out</text></g>`;
    // outcomes
    s += `<g class="fx" data-stage="2">
      <rect x="855" y="52" width="215" height="44" rx="8" class="node-green"/>
      <text x="962" y="71" class="t t-green t-sm" text-anchor="middle">accepted: S_AB</text>
      <text x="962" y="87" class="t t-dim t-sm" text-anchor="middle">spend P1,P2 · sign A,B</text></g>
      <g class="fx" data-stage="2">
      <rect x="855" y="212" width="215" height="44" rx="8" class="node-dead"/>
      <text x="962" y="231" class="t t-red t-sm" text-anchor="middle">S_BC → TokenAlreadySpent</text>
      <text x="962" y="247" class="t t-dim t-sm" text-anchor="middle">no shares are ever written</text></g>
      <path class="fx-draw edge-green" data-stage="2" data-pktstage="2" id="csP5" data-pkt="#3ECF8E" data-delay="100" d="M795 128 C 830 110, 830 96, 855 76"/>
      <path class="fx-draw edge-dead" data-stage="2" d="M795 178 C 830 196, 830 212, 855 232"/>`;
    // apply note
    s += `<g class="fx" data-stage="3">
      <text x="550" y="292" class="t t-dim" text-anchor="middle">every honest member applies the same journal entry: verify proofs → mark spent → sign accepted outputs · replay idempotent</text></g>`;
    s += `</svg>`;
    body.innerHTML = s;
    prepDraw(body);
    const svg = body.querySelector("svg");
    return {
      labels: ["envelopes", "into the box", "one winner", "apply"],
      go(i, token) {
        applyStages(body, i);
        runPackets(body, svg, i, "consensus", token);
      },
    };
  });

  /* ═══════════════════════════════════════════════════════════
     FIGURE: transport — public star vs private mesh
     ═══════════════════════════════════════════════════════════ */
  define("transport", (body) => {
    const cx = 560, cy = 230, R = 260;
    const angles = [-50, -25, 0, 25, 50];
    const mem = angles.map((a, i) => ({
      id: i + 1,
      x: cx + R * Math.cos((a * Math.PI) / 180),
      y: cy + R * Math.sin((a * Math.PI) / 180),
    }));
    const wx = 145, wy = 230;
    // shorten a segment to stop `rim` units short of both endpoints
    function rim(x1, y1, x2, y2, r1, r2) {
      const dx = x2 - x1, dy = y2 - y1;
      const L = Math.hypot(dx, dy) || 1;
      return [x1 + (dx / L) * r1, y1 + (dy / L) * r1, x2 - (dx / L) * r2, y2 - (dy / L) * r2];
    }
    let s = `<svg viewBox="0 0 1100 500" id="svgTransport" role="img" aria-label="Two planes: public fan-out and private mesh">`;
    // private mesh edges (member-member)
    for (let a = 0; a < 5; a++) for (let b = a + 1; b < 5; b++) {
      const [x1, y1, x2, y2] = rim(mem[a].x, mem[a].y, mem[b].x, mem[b].y, 26, 26);
      s += `<path class="fx-draw edge-mesh" data-stage="0" d="M${x1} ${y1} L${x2} ${y2}"
        ${(a + 1 === b) ? `data-pktstage="9" data-pkt="#F5A524" data-delay="${(a * 130)}"` : ""}/>`;
    }
    // public edges (wallet-member)
    mem.forEach((m, i) => {
      const [x1, y1, x2, y2] = rim(wx, wy, m.x, m.y, 6, 27);
      s += `<path class="fx-draw edge-sky" data-stage="0" data-pktstage="9" id="tpOut${i}" data-pkt="#4CC2FF" data-delay="${i * 130}"
        d="M${x1} ${y1} L${x2} ${y2}"/>`;
    });
    // wallet node
    s += `<g class="fx" data-stage="0">
      <rect x="30" y="202" width="115" height="56" rx="10" class="node-sky"/>
      <text x="87" y="226" class="t t-title" text-anchor="middle">wallet</text>
      <text x="87" y="244" class="t t-dim t-sm" text-anchor="middle">the aggregator</text></g>`;
    // member nodes
    mem.forEach((m, i) => {
      s += `<g class="fx-pop" data-stage="0" id="tpM${i}">
        <circle cx="${m.x}" cy="${m.y}" r="24" class="pt-node"/>
        <text x="${m.x}" y="${m.y + 4}" class="t t-title" text-anchor="middle">M${m.id}</text></g>`;
    });
    // center badge
    s += `<g class="fx-pop" data-stage="2">
      <rect x="${cx - 96}" y="${cy - 22}" width="192" height="44" rx="10" class="node-solid"/>
      <text x="${cx}" y="${cy - 3}" class="t t-green t-sm" text-anchor="middle">accepted: S_AB</text>
      <text x="${cx}" y="${cy + 13}" class="t t-dim t-sm" text-anchor="middle" style="font-size:8.5px">the exact envelope, ordered once</text></g>`;
    // offline marker on M4 (index 3)
    s += `<g class="fx-pop" data-stage="3">
      <circle cx="${mem[3].x}" cy="${mem[3].y}" r="30" class="pt-off"/>
      <text x="${mem[3].x + 38}" y="${mem[3].y + 18}" class="t t-dim t-sm">offline — tolerated</text></g>`;
    // return share paths (from all but M4)
    [0, 1, 2, 4].forEach((idx, k) => {
      const m = mem[idx];
      const [x1, y1, x2, y2] = rim(m.x, m.y, wx, wy, 27, 6);
      const mx = (x1 + x2) / 2, my = (y1 + y2) / 2;
      const dx = x2 - x1, dy = y2 - y1;
      const L = Math.hypot(dx, dy) || 1;
      s += `<path class="edge-ret" data-stage="3" data-pktstage="9" id="tpRet${idx}" data-pkt="#3ECF8E" data-delay="${k * 150}"
        d="M${x1} ${y1} Q${mx + (-dy / L) * 26} ${my + (dx / L) * 26} ${x2} ${y2}"/>`;
    });
    // proof chip at wallet
    s += `<g class="fx-pop" data-stage="3">
      <rect x="8" y="292" width="170" height="34" rx="8" class="node-green"/>
      <text x="93" y="313" class="t t-green t-sm" text-anchor="middle">one ordinary v3 proof</text></g>`;
    // legend
    s += `<g class="fx" data-stage="0">
      <path d="M30 472 L70 472" class="edge-sky"/><text x="78" y="476" class="t t-dim t-sm">public plane · ordinary Cashu over HTTPS or iroh</text>
      <path d="M470 472 L510 472" class="edge-mesh" style="stroke-dasharray:none"/><text x="518" y="476" class="t t-dim t-sm">private plane · /federation/v1 · consensus + DKG, member-authenticated</text></g>`;
    s += `</svg>`;
    body.innerHTML = s;
    prepDraw(body);
    const svg = body.querySelector("svg");
    return {
      labels: ["topology", "fan-out", "agree", "shares → proof"],
      go(i, token) {
        applyStages(body, i);
        if (i === 1) body.querySelectorAll(".edge-sky[data-pktstage]").forEach((p) => p.dataset.pktstage = "1");
        else body.querySelectorAll(".edge-sky[data-pktstage]").forEach((p) => p.dataset.pktstage = "9");
        if (i === 2) body.querySelectorAll(".edge-mesh[data-pktstage]").forEach((p) => p.dataset.pktstage = "2");
        else if (i < 2) body.querySelectorAll(".edge-mesh[data-pktstage]").forEach((p) => p.dataset.pktstage = "9");
        if (i === 3) body.querySelectorAll(".edge-ret[data-pktstage]").forEach((p) => p.dataset.pktstage = "3");
        else if (i < 3) body.querySelectorAll(".edge-ret[data-pktstage]").forEach((p) => p.dataset.pktstage = "9");
        runPackets(body, svg, i, "transport", token);
      },
    };
  });

  /* ═══════════════════════════════════════════════════════════
     FIGURE: BLS DKG ceremony lanes
     ═══════════════════════════════════════════════════════════ */
  define("dkg", (body) => {
    const cols = [290, 620, 950];
    let s = `<svg viewBox="0 0 1100 410" id="svgDkg" role="img" aria-label="BLS DKG ceremony">`;
    // phase labels
    ["1 · hash-commit", "2 · reveal", "3 · private delivery", "4 · sum"].forEach((t, i) => {
      s += `<text x="30" y="${[64, 134, 218, 324][i]}" class="t t-dim t-sm">${t}</text>`;
    });
    // member headers
    cols.forEach((x, i) => {
      s += `<g class="fx" data-stage="0"><circle cx="${x}" cy="24" r="15" class="pt-node"/>
        <text x="${x}" y="28" class="t t-sm t-title" text-anchor="middle">M${i + 1}</text></g>`;
    });
    // commit chips
    cols.forEach((x, i) => {
      s += `<g class="fx" data-stage="0"><rect x="${x - 115}" y="48" width="230" height="34" rx="8" class="chip"/>
        <text x="${x}" y="69" class="t t-sm" text-anchor="middle">publish H(reveal${i + 1}) <tspan class="t-dim">· no adapting</tspan></text></g>`;
    });
    // reveal chips
    cols.forEach((x, i) => {
      s += `<g class="fx" data-stage="1"><rect x="${x - 115}" y="118" width="230" height="34" rx="8" class="chip"/>
        <text x="${x}" y="139" class="t t-sm" text-anchor="middle">open G2 commits <tspan class="t-amber">C${i + 1}[0..t)</tspan></text></g>`;
    });
    // delivery arrows (j -> i): parallel lanes, sequence-chart style
    const lanes = [
      { from: 0, to: 1, y: 174 },
      { from: 1, to: 0, y: 192 },
      { from: 1, to: 2, y: 210 },
      { from: 2, to: 1, y: 228 },
      { from: 0, to: 2, y: 246 },
      { from: 2, to: 0, y: 264 },
    ];
    lanes.forEach(({ from, to, y }, k) => {
      const x1 = cols[from], x2 = cols[to];
      s += `<path class="fx-draw edge-amber" data-stage="2" data-pktstage="2" data-pkt="#F5A524" data-delay="${k * 110}"
        d="M${x1} ${y} L${x2} ${y}" id="dkgA${k}"/>`;
      s += `<text class="fx" data-stage="2" x="${(x1 + x2) / 2}" y="${y - 5}" class="t t-dim t-sm" text-anchor="middle">f${from + 1}(${to + 1})</text>`;
    });
    // verify note
    s += `<g class="fx" data-stage="2"><text x="620" y="286" class="t t-dim t-sm" text-anchor="middle">each evaluation is checked: fⱼ(i)·G2 == eval of j's public commitments at i — a lie is visible</text></g>`;
    // sum row
    cols.forEach((x, i) => {
      s += `<g class="fx" data-stage="3"><rect x="${x - 125}" y="306" width="250" height="36" rx="8" class="chip-hot"/>
        <text x="${x}" y="328" class="t t-sm" text-anchor="middle">k${i + 1} = f₁(${i + 1}) + f₂(${i + 1}) + f₃(${i + 1})</text></g>`;
    });
    s += `<g class="fx" data-stage="3">
      <rect x="330" y="360" width="580" height="36" rx="8" class="node-green"/>
      <text x="620" y="383" class="t t-green t-sm" text-anchor="middle">K = Σ Cⱼ[0] = k·G2 — wallets see one ordinary v3 keyset</text></g>`;
    s += `</svg>`;
    body.innerHTML = s;
    prepDraw(body);
    const svg = body.querySelector("svg");
    return {
      labels: ["commit", "reveal", "deliver", "sum"],
      go(i, token) { applyStages(body, i); runPackets(body, svg, i, "dkg", token); },
    };
  });

  /* ═══════════════════════════════════════════════════════════
     FIGURE: SIG_ALL hole — Byzantine member race
     ═══════════════════════════════════════════════════════════ */
  define("sigall", (body) => {
    const memX = [250, 400, 550, 700, 850];
    let s = `<svg viewBox="0 0 1100 410" id="svgSigall" role="img" aria-label="Byzantine member rewrites the swap">`;
    // wallet
    s += `<g class="fx" data-stage="0"><rect x="455" y="24" width="190" height="54" rx="10" class="node-sky"/>
      <text x="550" y="48" class="t t-title" text-anchor="middle">wallet</text>
      <text x="550" y="66" class="t t-dim t-sm" text-anchor="middle">swap P1,P2 → A,B — fanned to all</text></g>`;
    // fan-out edges
    memX.forEach((x, i) => {
      s += `<path class="fx-draw edge-sky" data-stage="0" data-pktstage="0" data-pkt="#4CC2FF" data-delay="${i * 110}"
        d="M550 78 L ${x} 138" id="saFan${i}"/>`;
    });
    // members
    memX.forEach((x, i) => {
      const evil = i === 4;
      s += `<g class="fx-pop" data-stage="${evil ? 1 : 0}">
        <circle cx="${x}" cy="162" r="22" class="${evil ? "pt-evil" : "pt-node"}"/>
        <text x="${x}" y="166" class="t t-sm t-title" text-anchor="middle">M${i + 1}</text></g>`;
    });
    // evil annotation chip
    s += `<g class="fx" data-stage="1">
      <rect x="760" y="204" width="330" height="50" rx="8" class="node-red"/>
      <text x="925" y="224" class="t t-red t-sm" text-anchor="middle">M5 copies the bearer proofs</text>
      <text x="925" y="241" class="t t-sm" text-anchor="middle">builds P1,P2 → X,Y · member-signed</text></g>`;
    // honest + attacker race paths to the box
    s += `<path class="fx-draw edge-sky" data-stage="2" data-pktstage="2" data-pkt="#4CC2FF" data-delay="350" data-dur="850"
        d="M250 184 C 300 268, 360 296, 424 306" id="saHonest"/>`;
    s += `<path class="fx-draw edge-red" data-stage="2" data-pktstage="2" data-pkt="#F4586B" data-delay="0" data-dur="520"
        d="M876 254 C 820 300, 740 312, 676 316" id="saRace"/>`;
    // consensus box
    s += `<g class="fx" data-stage="2"><rect x="424" y="284" width="252" height="64" rx="12" class="node-box"/>
      <text x="550" y="309" class="t t-title" text-anchor="middle">consensus box</text>
      <text x="550" y="327" class="t t-dim t-sm" text-anchor="middle">accepts the first well-formed op</text></g>`;
    // winner chip
    s += `<g class="fx" data-stage="2">
      <rect x="424" y="222" width="252" height="44" rx="8" class="node-red"/>
      <text x="550" y="241" class="t t-red t-sm" text-anchor="middle">winner: P1,P2 → X,Y</text>
      <text x="550" y="257" class="t t-dim t-sm" text-anchor="middle">balanced · well-formed · member-authorized</text></g>`;
    // outcome
    s += `<g class="fx" data-stage="3">
      <text x="550" y="384" class="t t-sm" text-anchor="middle">honest members spend P1,P2 and sign <tspan class="t-red">X, Y</tspan> — the wallet's {A, B} → <tspan class="t-red">TokenAlreadySpent</tspan></text></g>`;
    // SIG_ALL fix chip
    s += `<g class="fx" data-stage="3">
      <rect x="700" y="24" width="386" height="52" rx="8" class="node-green"/>
      <text x="893" y="44" class="t t-green t-sm" text-anchor="middle">SIG_ALL: the witness commits to the exact outputs</text>
      <text x="893" y="62" class="t t-dim t-sm" text-anchor="middle">mutation invalidates the signature and changes the operation id</text></g>`;
    s += `</svg>`;
    body.innerHTML = s;
    prepDraw(body);
    const svg = body.querySelector("svg");
    return {
      labels: ["fan-out", "one evil member", "race", "attacker outputs win"],
      go(i, token) { applyStages(body, i); runPackets(body, svg, i, "sigall", token); },
    };
  });

  /* ═══════════════════════════════════════════════════════════
     FIGURE: FROST + BLS dual DKG
     ═══════════════════════════════════════════════════════════ */
  define("frost", (body) => {
    let s = `<svg viewBox="0 0 1100 350" id="svgFrost" role="img" aria-label="Two DKG ceremonies">`;
    s += `<g class="fx" data-stage="0">
      <text x="550" y="30" class="t t-dim" text-anchor="middle">one roster · one ceremony id · one signature threshold — the secrets never mix</text></g>`;
    // BLS lane
    s += `<g class="fx" data-stage="1">
      <text x="40" y="76" class="t t-amber t-title">BLS12-381 DKG</text>
      <text x="40" y="94" class="t t-dim t-sm">Cashu eCash keysets</text></g>`;
    const blsChips = ["commit → reveal → deliver", "k_i per member", "K = k·G2", "signs blinded Cashu proofs"];
    blsChips.forEach((t, i) => {
      s += `<g class="fx" data-stage="1"><rect x="${300 + i * 195}" y="60" width="180" height="40" rx="8" class="${i === 3 ? "chip-hot" : "chip"}"/>
        <text x="${390 + i * 195}" y="84" class="t t-sm" text-anchor="middle">${t}</text></g>`;
    });
    // FROST lane
    s += `<g class="fx" data-stage="2">
      <text x="40" y="176" class="t t-violet t-title">secp256k1 FROST DKG</text>
      <text x="40" y="194" class="t t-dim t-sm">treasury root · frost-secp256k1-tr</text></g>`;
    const frostChips = ["round 1 · broadcast commits", "round 2 · confidential shares", "root confirm · all agree", "P — untweaked root"];
    frostChips.forEach((t, i) => {
      s += `<g class="fx" data-stage="2"><rect x="${300 + i * 195}" y="160" width="180" height="40" rx="8" class="${i === 3 ? "chip-violet" : "chip"}"/>
        <text x="${390 + i * 195}" y="184" class="t t-sm" text-anchor="middle">${t}</text></g>`;
    });
    // divider
    s += `<path class="fx-draw edge" data-stage="2" d="M270 40 L270 210"/>`;
    // derivation order
    s += `<g class="fx" data-stage="3">
      <text x="40" y="252" class="t t-dim t-sm">apps derive later, in order:</text>
      <text x="40" y="274" class="t t-sm">untweaked root → domain tweak → chain code → non-hardened BIP32 → BIP340 even-Y → BIP341 key-path tweak</text>
      <rect x="40" y="296" width="620" height="38" rx="8" class="node-green"/>
      <text x="350" y="319" class="t t-green t-sm" text-anchor="middle">setup completes only when both ceremonies exist and every member agrees on both public results</text></g>`;
    s += `<g class="fx" data-stage="3">
      <rect x="700" y="296" width="360" height="38" rx="8" class="chip-violet"/>
      <text x="880" y="319" class="t t-sm" text-anchor="middle">any t members → ordinary Schnorr under P</text></g>`;
    s += `</svg>`;
    body.innerHTML = s;
    prepDraw(body);
    return {
      labels: ["one roster", "BLS lane", "FROST lane", "both or neither"],
      go(i) { applyStages(body, i); },
    };
  });

  /* ═══════════════════════════════════════════════════════════
     FIGURE: BDK mint / melt consensus objects
     ═══════════════════════════════════════════════════════════ */
  define("bdk", (body) => {
    const mint = [
      ["MintQuote", "val", 0], ["AddressAllocation", "cust", 0], ["D × Deposit obs", "obs", 1],
      ["Q × MintQuotePayment", "obs", 1], ["Mint · exact outputs", "val", 1], ["T × BLS shares → wallet", "ok", 1],
    ];
    const melt = [
      ["MeltQuote", "val", 2], ["Melt · proofs + intent", "val", 2], ["TxProposal", "cust", 3],
      ["FROST rounds · off-log", "cust", 3], ["TxSigned", "cust", 3], ["BroadcastIntent", "cust", 3],
      ["B,Q × obs → paid", "obs", 3],
    ];
    function row(items, y) {
      let x = 0, out = "";
      items.forEach(([t, kind, st], i) => {
        const w = t.length * 6.0 + 24;
        out += `<g class="fx" data-stage="${st}">
          <rect x="${x}" y="${y}" width="${w}" height="34" rx="8" class="chip-${kind}"/>
          <text x="${x + w / 2}" y="${y + 21}" class="t t-sm" text-anchor="middle" style="font-size:9px">${t}</text></g>`;
        if (i < items.length - 1) {
          const nst = items[i + 1][2];
          out += `<path class="fx-draw edge" data-stage="${Math.max(st, nst)}" d="M${x + w} ${y + 17} L${x + w + 10} ${y + 17}"/>`;
        }
        x += w + 10;
      });
      return out;
    }
    let s = `<svg viewBox="0 0 1100 240" id="svgBdk" role="img" aria-label="Consensus objects behind BDK mint and melt">`;
    s += `<g class="fx" data-stage="0"><text x="30" y="34" class="t t-amber t-title">MINT</text>
      <text x="30" y="140" class="t t-violet t-title">MELT</text></g>`;
    s += `<g transform="translate(96,0)">${row(mint, 14)}</g>`;
    s += `<g transform="translate(96,0)">${row(melt, 120)}</g>`;
    s += `<g class="fx" data-stage="4">
      <text x="550" y="186" class="t t-dim t-sm" text-anchor="middle">mempool is not payment · reorg before issuance unsays the UTXO · reorg after issuance alarms, never unmints · one member cannot redirect the payout</text></g>`;
    // legend
    s += `<g class="fx" data-stage="0">
      <rect x="330" y="208" width="12" height="12" rx="3" class="lg-val"/><text x="348" y="218" class="t t-dim t-sm">value op</text>
      <rect x="430" y="208" width="12" height="12" rx="3" class="lg-obs"/><text x="448" y="218" class="t t-dim t-sm">observation</text>
      <rect x="546" y="208" width="12" height="12" rx="3" class="lg-cust"/><text x="564" y="218" class="t t-dim t-sm">custody</text>
      <rect x="646" y="208" width="12" height="12" rx="3" class="lg-ok"/><text x="664" y="218" class="t t-dim t-sm">to wallet</text></g>`;
    s += `</svg>`;
    body.innerHTML = s;
    prepDraw(body);
    return {
      labels: ["quote + address", "observe → paid", "melt intent", "tx → FROST → broadcast", "rules"],
      go(i) { applyStages(body, i); },
    };
  });

  /* ═══════════════════════════════════════════════════════════
     FIGURE: swap anatomy
     ═══════════════════════════════════════════════════════════ */
  define("swap", (body) => {
    const steps = [
      ["wallet", "val2", "POST /swap to everyone"],
      ["envelope", "val2", "kind: Swap · in/outputs"],
      ["operation_id", "dim2", "hash(envelope)"],
      ["consensus", "hot2", "one winner accepted"],
      ["apply", "hot2", "verify · spend · sign"],
      ["shares", "ok2", "C_i = k_i·B → one C"],
    ];
    let s = `<svg viewBox="0 0 1100 130" id="svgSwap" role="img" aria-label="Federated swap pipeline">`;
    let x = 18;
    steps.forEach(([t, kind, sub], i) => {
      const w = 166;
      s += `<g class="fx" data-stage="${i}">
        <rect x="${x}" y="22" width="${w}" height="62" rx="9" class="chip-${kind}"/>
        <text x="${x + w / 2}" y="47" class="t t-sm t-title" text-anchor="middle">${t}</text>
        <text x="${x + w / 2}" y="68" class="t t-dim" text-anchor="middle" style="font-size:8.6px">${sub}</text></g>`;
      if (i < steps.length - 1) s += `<path class="fx-draw edge" data-stage="${i + 1}" d="M${x + w} 53 L${x + w + 14} 53"/>`;
      x += w + 14;
    });
    s += `<g class="fx" data-stage="5"><text x="550" y="114" class="t t-dim t-sm" text-anchor="middle">replay of the same accepted swap returns the same shares · a different output set for spent inputs dies with TokenAlreadySpent</text></g>`;
    s += `</svg>`;
    body.innerHTML = s;
    prepDraw(body);
    return {
      labels: steps.map(([t]) => t),
      go(i) { applyStages(body, i); },
    };
  });

  /* ═══════════════════════════════════════════════════════════
     FIGURE: happy-path operation counts (stacked bars)
     ═══════════════════════════════════════════════════════════ */
  define("counts", (body) => {
    const rows = [
      { label: "Swap — any backend", total: 1, stage: 0, seg: [["value", 1]] },
      { label: "Mint — fakewallet", total: 3, stage: 0, seg: [["value", 2], ["obs", 1]] },
      { label: "Melt — fakewallet", total: 3, stage: 0, seg: [["value", 2], ["obs", 1]] },
      { label: "Mint — federated BDK", total: 11, stage: 1, seg: [["value", 2], ["custody", 1], ["obs", 8]] },
      { label: "Melt — federated BDK", total: 11, stage: 2, seg: [["value", 2], ["custody", 3], ["obs", 6]] },
      { label: "Mint — federated Bark", total: 16, stage: 3, seg: [["value", 2], ["app", 6], ["obs", 8]] },
    ];
    const MAX = 16;
    let h = `<div class="counts" role="img" aria-label="Accepted operation counts per flow">`;
    rows.forEach((r) => {
      h += `<div class="count-row fx" data-stage="${r.stage}">
        <span class="count-label">${r.label}</span>
        <span class="count-bar">`;
      r.seg.forEach(([kind, n]) => {
        h += `<span class="count-seg seg-${kind}" style="width:${(n / MAX) * 100}%"></span>`;
      });
      h += `</span><span class="count-n">${r.total}</span></div>`;
    });
    h += `<div class="count-legend fx" data-stage="3">
      <span><i class="seg-value"></i>value op</span><span><i class="seg-obs"></i>observation (one per member)</span>
      <span><i class="seg-custody"></i>custody</span><span><i class="seg-app"></i>Bark application</span>
      <span class="t-dim">distinct accepted operations — duplicates join, they don't stack · T× BLS/FROST shares live off-log</span></div></div>`;
    body.innerHTML = h;
    return {
      labels: ["swap + fakewallet", "BDK mint ≈ 11", "BDK melt ≈ 11", "Bark ≈ 16"],
      go(i) { applyStages(body, i); },
    };
  });

  /* ═══════════════════════════════════════════════════════════
     FIGURE: federation_id — live SHA-256 demo
     ═══════════════════════════════════════════════════════════ */
  define("roster", (body) => {
    const members = [
      { id: 1, url: "https://mint-a.example", fed: "https://mint-a.example/federation/v1", key: "02c6047f9441ed7d6d…" },
      { id: 2, url: "https://mint-b.example", fed: "https://mint-b.example/federation/v1", key: "03f9308a019258c310…" },
      { id: 3, url: "iroh://9f3b2a1c…", fed: "iroh://9f3b2a1c…/federation/v1", key: "02e493dbf1c10d80f9…" },
    ];
    let h = `<div class="roster">
      <div class="roster-fields">
        <div class="roster-row"><span class="roster-k">setup_authorization</span><span class="roster-v">unanimous proposal · 5d3f…a9</span></div>
        <div class="roster-row"><span class="roster-k">thresholds</span><span class="roster-v">n=3 · T=2 · C=3</span></div>
        ${members.map((m, i) => `
        <div class="roster-row roster-member">
          <span class="roster-k">member ${m.id}</span>
          <span class="roster-v">
            <input class="roster-url" data-m="${i}" value="${m.url}" spellcheck="false" aria-label="member ${m.id} public URL">
            <span class="roster-sub">${m.fed} · ${m.key}</span>
          </span>
        </div>`).join("")}
      </div>
      <div class="roster-mid"><span class="roster-sha">SHA-256 · domain-separated transcript</span><span class="roster-arrow">⟶</span></div>
      <div class="roster-out">
        <p class="roster-out-k">federation_id</p>
        <p class="roster-id" id="rosterId">…</p>
        <p class="roster-status" id="rosterStatus"></p>
        <button class="replay roster-reset" id="rosterReset" title="Reset URLs">↺ reset</button>
      </div>
    </div>`;
    body.innerHTML = h;

    const idEl = body.querySelector("#rosterId");
    const statusEl = body.querySelector("#rosterStatus");
    const inputs = Array.from(body.querySelectorAll(".roster-url"));
    let baseline = null;

    async function sha256hex(text) {
      if (window.crypto && crypto.subtle) {
        const buf = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
        return Array.from(new Uint8Array(buf)).map((b) => b.toString(16).padStart(2, "0")).join("");
      }
      // fallback (non-secure context): cyrb53, still demonstrates the point
      let h1 = 0xdeadbeef, h2 = 0x41c6ce57;
      for (let i = 0; i < text.length; i++) {
        const ch = text.charCodeAt(i);
        h1 = Math.imul(h1 ^ ch, 2654435761);
        h2 = Math.imul(h2 ^ ch, 1597334677);
      }
      h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ^ Math.imul(h2 ^ (h2 >>> 13), 3266489909);
      h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507) ^ Math.imul(h1 ^ (h1 >>> 13), 3266489909);
      return (h2 >>> 0).toString(16).padStart(8, "0") + (h1 >>> 0).toString(16).padStart(8, "0");
    }

    function transcript() {
      const urls = inputs.map((inp) => inp.value);
      return ["CDK_FEDERATION_ID_V1", "setup:5d3fa9", "n=3,T=2,C=3",
        ...members.map((m, i) => `${m.id}|${urls[i]}|${m.fed}|${m.key}`)].join("\n");
    }

    let flipTimer = null;
    async function recompute(initial) {
      const hex = await sha256hex(transcript());
      const short = hex.slice(0, 16) + "…";
      if (initial) baseline = hex;
      const same = hex === baseline;
      idEl.textContent = short;
      idEl.classList.remove("flip");
      void idEl.offsetWidth;
      idEl.classList.add("flip");
      statusEl.textContent = same
        ? "roster match — wallets will aggregate"
        : "different federation_id — this is a new mint";
      statusEl.classList.toggle("bad", !same);
      clearTimeout(flipTimer);
    }

    inputs.forEach((inp) => inp.addEventListener("input", () => recompute(false)));
    body.querySelector("#rosterReset").addEventListener("click", () => {
      inputs.forEach((inp, i) => (inp.value = members[i].url));
      recompute(false);
    });
    recompute(true);

    return { labels: [], go() {} };
  });
})();
