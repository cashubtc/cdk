(() => {
    const NS = "http://www.w3.org/2000/svg";
    const reduce = () =>
        window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    const figures = new Map();

    function noteOf(root) {
        return root.querySelector("[data-note]");
    }

    function setNote(root, text) {
        const n = noteOf(root);
        if (n) n.textContent = text;
    }

    function bindControls(root, api) {
        root.addEventListener("click", (e) => {
            const btn = e.target.closest("[data-act]");
            if (!btn || !root.contains(btn)) return;
            const act = btn.dataset.act;
            if (act === "next") api.go(api.step + 1);
            if (act === "prev") api.go(api.step - 1);
            if (act === "play") api.play();
            if (act === "reset") api.go(0);
            if (act === "plane") {
                root.dataset.plane = btn.dataset.plane;
                root.querySelectorAll("[data-act='plane']").forEach((b) => {
                    b.classList.toggle("is-on", b === btn);
                    b.setAttribute("aria-pressed", b === btn ? "true" : "false");
                });
            }
            if (act === "pipe") {
                root.dataset.pipe = btn.dataset.pipe;
                root.querySelectorAll("[data-act='pipe']").forEach((b) => {
                    b.classList.toggle("is-on", b === btn);
                    b.setAttribute("aria-pressed", b === btn ? "true" : "false");
                });
                if (typeof api.onPipe === "function") api.onPipe(btn.dataset.pipe);
            }
            if (act === "mode") {
                root.dataset.mode = btn.dataset.mode;
                root.querySelectorAll("[data-act='mode']").forEach((b) => {
                    b.classList.toggle("is-on", b === btn);
                    b.setAttribute("aria-pressed", b === btn ? "true" : "false");
                });
                api.go(0);
                if (typeof api.onMode === "function") api.onMode(btn.dataset.mode);
            }
        });
    }

    function stepper(root, { max, notes, render, interval = 900 }) {
        const api = {
            step: 0,
            timer: 0,
            go(n) {
                api.step = Math.max(0, Math.min(max, n));
                root.dataset.step = String(api.step);
                setNote(root, notes[api.step] || "");
                render(api.step, root);
            },
            play() {
                window.clearTimeout(api.timer);
                if (api.step >= max) api.go(0);
                const tick = () => {
                    if (api.step >= max) return;
                    api.go(api.step + 1);
                    api.timer = window.setTimeout(tick, interval);
                };
                api.timer = window.setTimeout(tick, reduce() ? 0 : 180);
            },
        };
        bindControls(root, api);
        api.go(0);
        figures.set(root.id, api);
        return api;
    }

    function svgEl(name, attrs) {
        const el = document.createElementNS(NS, name);
        for (const [k, v] of Object.entries(attrs || {})) el.setAttribute(k, v);
        return el;
    }

    function travel(svg, path, { color = "#c05621", duration = 700 } = {}) {
        if (!path || reduce()) return Promise.resolve();
        const g = svgEl("g", { class: "packet" });
        g.appendChild(svgEl("circle", { r: "4.5", fill: color, cx: "0", cy: "0" }));
        svg.appendChild(g);
        const len = path.getTotalLength();
        return new Promise((resolve) => {
            const start = performance.now();
            const frame = (now) => {
                const t = Math.min(1, (now - start) / duration);
                const pt = path.getPointAtLength(len * t);
                g.setAttribute("transform", `translate(${pt.x} ${pt.y})`);
                if (t < 1) {
                    requestAnimationFrame(frame);
                } else {
                    g.remove();
                    resolve();
                }
            };
            requestAnimationFrame(frame);
        });
    }

    function initDhke() {
        const root = document.getElementById("fig-dhke");
        if (!root) return;
        const notes = [
            "Wallet hashes the secret into G1, then blinds with r.",
            "Mint multiplies by k. No DLEQ. Same HTTP BlindedMessage.",
            "Wallet checks the blind pairing before it unblinds.",
            "Unblind, then the ordinary proof pairing. Token stays Cashu.",
        ];
        const ids = ["s0", "s1", "s2", "s3"];
        stepper(root, {
            max: 3,
            notes,
            render(step) {
                ids.forEach((id, i) => {
                    const el = root.querySelector(`[data-pipe='${id}']`);
                    if (!el) return;
                    el.classList.toggle("is-on", i === step);
                    el.classList.toggle("is-done", i < step);
                });
            },
        });
    }

    function initThreshold() {
        const root = document.getElementById("fig-threshold");
        if (!root) return;
        const notes = [
            "Line f(x) = 7 + 3x. Secret is f(0) = 7. IDs are 1, 2, 3 — never 0.",
            "Any two shares interpolate to 7B. {1,2}: 2·10B − 13B = 7B.",
            "{2,3}: 3·13B − 2·16B = 7B. Same C. One share is useless.",
            "Unblind 7B with r. Proof verifies against K = 7·G2.",
        ];
        const svg = root.querySelector("svg");
        stepper(root, {
            max: 3,
            notes,
            render(step) {
                if (!svg) return;
                svg.querySelectorAll("[data-lag]").forEach((el) => {
                    const on = Number(el.dataset.lag) <= step;
                    el.style.opacity = on ? "1" : "0";
                });
            },
        });
    }

    function paintMix(root, step, mode) {
        const cells = {
            A1: step >= 1,
            B1: step >= 1,
            C1: false,
            A2: step >= 1,
            B2: step >= 1,
            C2: false,
            A3: step >= 1,
            B3: step >= 1,
            C3: mode === "attack" ? step >= 2 : false,
            A4: false,
            B4: step >= 1,
            C4: step >= 1,
            A5: false,
            B5: step >= 1,
            C5: step >= 1,
        };
        if (mode === "consensus") {
            Object.keys(cells).forEach((k) => {
                if (k.startsWith("C")) cells[k] = false;
            });
        }
        root.querySelectorAll("[data-cell]").forEach((el) => {
            const key = el.dataset.cell;
            const on = Boolean(cells[key]);
            el.classList.toggle("on", on);
            const col = key[0];
            const count = ["1", "2", "3", "4", "5"].filter((m) => cells[`${col}${m}`]).length;
            el.classList.toggle("win", on && count >= 3);
            el.classList.toggle("dead", mode === "consensus" && col === "C" && step >= 2);
        });
        ["A", "B", "C"].forEach((col) => {
            const count = ["1", "2", "3", "4", "5"].filter((m) => {
                const el = root.querySelector(`[data-cell='${col}${m}']`);
                return el && el.classList.contains("on");
            }).length;
            const tag = root.querySelector(`[data-col='${col}']`);
            if (tag) tag.textContent = `${count}/3 ${count >= 3 ? "aggregates" : ""}`;
        });
    }

    function initMix() {
        const root = document.getElementById("fig-mix");
        if (!root) return;
        const notesAttack = [
            "Paid quote authorizes 2 notes. Attacker prepared A, B, and extra C.",
            "First window: M1–M3 see {A,B}. M4–M5 see {B,C}. C has only 2 shares.",
            "Slide: M3 also signs {B,C}. Every column now has 3 stars.",
            "Paid 2, minted 3. Independently valid shares were enough.",
        ];
        const notesCons = [
            "Same inputs, two envelopes: S_AB and S_BC. Different hashes.",
            "The box accepts one winner. Here S_AB. Inputs P1,P2 are spent.",
            "C is not in the accepted operation. Nobody signs it.",
            "Later S_BC dies with TokenAlreadySpent. No shares for C.",
        ];
        const api = stepper(root, {
            max: 3,
            notes: notesAttack,
            render(step) {
                const mode = root.dataset.mode || "attack";
                paintMix(root, step, mode);
                setNote(root, (mode === "attack" ? notesAttack : notesCons)[step]);
            },
        });
        api.onMode = () => {
            const mode = root.dataset.mode || "attack";
            setNote(root, (mode === "attack" ? notesAttack : notesCons)[0]);
        };
        root.dataset.mode = "attack";
        api.go(1);
    }

    function initTransport() {
        const root = document.getElementById("fig-transport");
        if (!root) return;
        const svg = root.querySelector("svg");
        const notes = [
            "Two planes, same roster. Public star for Cashu. Private mesh for agreement.",
            "Wallet fans the same SwapRequest to every public_mint_url.",
            "Each ready member hashes the envelope and submits it privately.",
            "AlephBFT emits one total order. S_AB is accepted. Not S_BC.",
            "Shares return only for accepted outputs. M4 can be offline. Any 3 suffice.",
            "Wallet checks e(C_i, G2) == e(B, K_i), interpolates one C, unblinds.",
        ];

        function setPipe(pipe) {
            const http = pipe !== "iroh";
            svg.querySelectorAll("[data-url]").forEach((el) => {
                const kind = el.dataset.url;
                el.textContent =
                    kind === "public"
                        ? http
                            ? "https://mint-n/…"
                            : "iroh://<endpoint>"
                        : http
                          ? "/federation/v1"
                          : "iroh · /federation/v1";
            });
        }

        const api = stepper(root, {
            max: 5,
            notes,
            interval: 1100,
            render(step) {
                svg.querySelectorAll(".edge.public:not(.share)").forEach((e) => {
                    e.classList.toggle("dim", step === 0 || step === 2 || step === 3);
                    e.classList.toggle("live", step === 1 || step >= 4);
                });
                svg.querySelectorAll(".edge.private").forEach((e) => {
                    e.classList.toggle("dim", step <= 1 || step >= 4);
                    e.classList.toggle("live", step === 2 || step === 3);
                });
                svg.querySelector("#m4")?.classList.toggle("is-off", step >= 4);
                svg.querySelector("#cbox")?.classList.toggle("is-hot", step === 3);
                if (step === 1 && !reduce()) {
                    svg.querySelectorAll("path.edge.public:not(.share)").forEach((p) => {
                        travel(svg, p, { color: "#2c5d86", duration: 640 });
                    });
                }
                if (step === 2 && !reduce()) {
                    svg.querySelectorAll("path.edge.to-box").forEach((p) => {
                        travel(svg, p, { color: "#5a3472", duration: 700 });
                    });
                }
                if (step === 4 && !reduce()) {
                    svg.querySelectorAll("path.edge.share").forEach((p) => {
                        travel(svg, p, { color: "#c05621", duration: 700 });
                    });
                }
            },
        });
        api.onPipe = setPipe;
        root.dataset.plane = "both";
        root.dataset.pipe = "http";
        setPipe("http");
    }

    function initDkg() {
        const root = document.getElementById("fig-dkg");
        if (!root) return;
        const math = root.querySelector("[data-math]");
        const blocks = [
            "Agree first: roster, t=2, unit, amounts, fees, expiry. If those differ, the ceremony cannot finish.",
            "Each member picks f_j and immediately evaluates shares for every receiver. Raw coefficients are thrown away.",
            "Hash-commit H(reveal) first. Stops the last dealer from adapting to everyone else's keys.",
            "Reveal G2 commitments. First coefficient is that member's public piece of K. Must match the hash.",
            "Private delivery: j sends f_j(i) on the authenticated plane. i checks f_j(i)·G2 against j's polynomial.",
            "Sum. M2 gets 6+12+5 = 23 = f(2). Nobody learns k = 7.",
            "K = C1[0]+C2[0]+C3[0] = 7·G2. That is the v3 keyset the wallet sees.",
            "Transcript + member signatures, then activate. Until then, nobody signs eCash with it.",
        ];
        stepper(root, {
            max: 7,
            notes: [
                "Dealer-free Pedersen DKG. Same end state as a trusted dealer. Production wants this so no machine holds k.",
                "Local polynomials only. Joint f(x) = 7 + 8x lives in nobody's memory.",
                "Commitments are hashes, not points. Peeking is useless.",
                "Opened G2 points. A lie later is visible.",
                "Envelopes on the private mesh, not wallet fan-out.",
                "k_i is the Shamir share used later as C_i = k_i · B.",
                "Wallets never see the dance. They see one mint public key.",
                "Activation is the last public step. Signing before this is forbidden.",
            ],
            interval: 1200,
            render(step) {
                if (math) math.innerHTML = blocks[step];
                root.querySelectorAll("[data-dkg]").forEach((el) => {
                    const from = Number(el.dataset.from || 0);
                    const only = el.dataset.only;
                    const on = only ? Number(only) === step : step >= from;
                    el.style.opacity = on ? "1" : "0";
                });
                root.querySelectorAll(".dkg-node").forEach((el) => {
                    el.classList.toggle("is-sealed", step >= 2 && step < 4);
                });
                if (step === 4 && !reduce()) {
                    const svg = root.querySelector("svg");
                    svg.querySelectorAll("path.dkg-e").forEach((p) => {
                        travel(svg, p, { color: "#c05621", duration: 800 });
                    });
                }
            },
        });
    }

    function initConsensus() {
        const root = document.getElementById("fig-consensus");
        if (!root) return;
        stepper(root, {
            max: 3,
            notes: [
                "A swap is a named envelope. Same bytes, same operation_id.",
                "Members put envelopes in. They do not sign because HTTP arrived.",
                "The box emits one total order. S_AB wins. Inputs are spent.",
                "Shares exist only after accept, and only for those outputs.",
            ],
            render(step) {
                root.querySelectorAll("[data-c]").forEach((el) => {
                    el.style.opacity = Number(el.dataset.c) <= step ? "1" : "0";
                });
            },
        });
    }

    function initSigall() {
        const root = document.getElementById("fig-sigall");
        if (!root) return;
        stepper(root, {
            max: 3,
            notes: [
                "Wallet fans P1,P2 → A,B. The request contains the bearer secrets.",
                "Evil M5 copies the proofs, rewrites outputs to X,Y, submits first.",
                "Consensus accepts the well-formed rewrite. Honest members sign X,Y.",
                "SIG_ALL would bind the owner to A,B. Mutation would fail apply.",
            ],
            render(step) {
                root.querySelectorAll("[data-s]").forEach((el) => {
                    el.style.opacity = Number(el.dataset.s) <= step ? "1" : "0";
                });
            },
        });
    }

    function initBdk() {
        const root = document.getElementById("fig-bdk");
        if (!root) return;
        stepper(root, {
            max: 4,
            notes: [
                "Quote + address allocation are consensus facts, not BDK reveal_next_address().",
                "Deposit observations need quorum and depth. Mempool is not payment.",
                "Mint envelope: exact outputs, conflict-keyed on the quote, then BLS shares.",
                "Melt does not send coins. It creates an intent, then a proposal, then FROST.",
                "Signed bytes persist before broadcast. Observations cannot swap another txid.",
            ],
            render(step) {
                root.querySelectorAll("[data-b]").forEach((el) => {
                    el.classList.toggle("is-on", Number(el.dataset.b) === step);
                    el.classList.toggle("is-done", Number(el.dataset.b) < step);
                });
            },
        });
    }

    function boot() {
        initDhke();
        initThreshold();
        initMix();
        initTransport();
        initDkg();
        initConsensus();
        initSigall();
        initBdk();
    }

    window.CDKDeckFigures = {
        boot,
        get(id) {
            return figures.get(id);
        },
        currentOn(slide) {
            const fig = slide && slide.querySelector(".figure[id]");
            return fig ? figures.get(fig.id) : null;
        },
    };
})();
