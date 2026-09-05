(() => {
    const slides = () => [...document.querySelectorAll(".slide")];

    function instantInto(el) {
        el.scrollIntoView({ behavior: "auto", block: "start" });
    }

    function currentIndex() {
        const all = slides();
        const y = window.scrollY + window.innerHeight * 0.35;
        let idx = 0;
        all.forEach((s, i) => {
            if (s.offsetTop <= y) idx = i;
        });
        return idx;
    }

    function go(i) {
        const all = slides();
        const next = all[Math.max(0, Math.min(all.length - 1, i))];
        if (!next) return;
        instantInto(next);
        mark(all.indexOf(next));
        history.replaceState(null, "", `#${next.id}`);
    }

    function mark(i) {
        const all = slides();
        all.forEach((s, n) => s.classList.toggle("is-current", n === i));
        document.querySelectorAll(".spine button").forEach((b, n) => {
            b.setAttribute("aria-current", n === i ? "true" : "false");
        });
        const slide = all[i];
        const kicker = document.getElementById("top-kicker");
        const count = document.getElementById("top-count");
        if (kicker && slide) {
            kicker.innerHTML = `<b>${slide.dataset.part || ""}</b> · ${slide.dataset.title || ""}`;
        }
        if (count) {
            const n = String(i + 1).padStart(2, "0");
            const t = String(all.length).padStart(2, "0");
            count.textContent = `${n} / ${t}`;
        }
    }

    function buildChrome() {
        const spine = document.getElementById("spine");
        const index = document.getElementById("index-list");
        slides().forEach((s, i) => {
            const b = document.createElement("button");
            b.type = "button";
            b.title = s.dataset.title || s.id;
            b.addEventListener("click", () => go(i));
            spine.appendChild(b);
            if (index) {
                const li = document.createElement("li");
                const a = document.createElement("a");
                a.href = `#${s.id}`;
                a.textContent = `${String(i).padStart(2, "0")}  ${s.dataset.title || s.id}`;
                a.addEventListener("click", (e) => {
                    e.preventDefault();
                    document.getElementById("index").classList.remove("is-open");
                    go(i);
                });
                li.appendChild(a);
                index.appendChild(li);
            }
        });
    }

    function onScroll() {
        mark(currentIndex());
    }

    function onKey(e) {
        if (e.metaKey || e.ctrlKey || e.altKey) return;
        const tag = document.activeElement && document.activeElement.tagName;
        if (tag === "INPUT" || tag === "TEXTAREA") return;

        if (e.key === "t" || e.key === "?") {
            e.preventDefault();
            document.getElementById("index").classList.toggle("is-open");
            return;
        }
        if (e.key === "Escape") {
            document.getElementById("index").classList.remove("is-open");
            return;
        }

        const indexOpen = document.getElementById("index").classList.contains("is-open");
        if (indexOpen) return;

        if (e.key === "n" || e.key === "p") {
            const fig = window.CDKDeckFigures.currentOn(slides()[currentIndex()]);
            if (!fig) return;
            e.preventDefault();
            if (e.key === "n") fig.go(fig.step + 1);
            else fig.go(fig.step - 1);
            return;
        }

        const nextKeys = ["ArrowDown", "PageDown", "j", " ", "ArrowRight"];
        const prevKeys = ["ArrowUp", "PageUp", "k", "ArrowLeft"];
        if (nextKeys.includes(e.key)) {
            e.preventDefault();
            go(currentIndex() + 1);
        } else if (prevKeys.includes(e.key)) {
            e.preventDefault();
            go(currentIndex() - 1);
        } else if (e.key === "Home") {
            e.preventDefault();
            go(0);
        } else if (e.key === "End") {
            e.preventDefault();
            go(slides().length - 1);
        }
    }

    function boot() {
        buildChrome();
        window.CDKDeckFigures.boot();
        const hash = location.hash.replace("#", "");
        const start = slides().findIndex((s) => s.id === hash);
        mark(start >= 0 ? start : 0);
        if (start > 0) go(start);
        window.addEventListener("scroll", onScroll, { passive: true });
        window.addEventListener("keydown", onKey);
        document.getElementById("index-close")?.addEventListener("click", () => {
            document.getElementById("index").classList.remove("is-open");
        });
    }

    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", boot);
    } else {
        boot();
    }
})();
