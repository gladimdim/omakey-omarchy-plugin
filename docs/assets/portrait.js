/* Portrait mode: the phone upright, the touchpad on top and the phone's own
   keyboard (Gboard, SwiftKey, any) below it, typing into the computer.
   Follows the app: ImeCapture keeps the keyboard's line and sends each
   change as arrows, backspaces and characters (LineDiff), Typist paces the
   strokes and picks a layout per character (KeyLayouts), KeyStrip has the
   keys a phone keyboard lacks, TypedTicker shows what went out. */
(function () {
  "use strict";
  const OMK = window.OMK;
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
  const el = (tag, cls, html) => { const e = document.createElement(tag); if (cls) e.className = cls; if (html != null) e.innerHTML = html; return e; };
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const reduced = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const SHORTCUT_MODS = ["KEY_LEFTCTRL", "KEY_LEFTALT", "KEY_LEFTMETA"];
  // The app sends a stroke every 8 ms; slowed here so an autocorrection is visible.
  const STROKE_MS = 30;

  /* ═══ Characters → keys, per computer layout (KeyLayouts.kt) ═══ */
  const CODES = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".split("").map((c) => "KEY_" + c).concat(
    ["KEY_MINUS", "KEY_EQUAL", "KEY_LEFTBRACE", "KEY_RIGHTBRACE", "KEY_BACKSLASH", "KEY_SEMICOLON", "KEY_APOSTROPHE", "KEY_GRAVE", "KEY_COMMA", "KEY_DOT", "KEY_SLASH", "KEY_SPACE"]);
  let LAYOUTS = null;
  const layouts = () => {
    if (LAYOUTS) return LAYOUTS;
    const build = (xkb) => {
      const m = new Map();
      for (const code of CODES) for (const shift of [false, true]) {
        const c = OMK.charFor(code, shift, false, xkb);
        if (c && !m.has(c)) m.set(c, [code, shift]);
      }
      m.set("\n", ["KEY_ENTER", false]);
      m.set("\t", ["KEY_TAB", false]);
      return { xkb, map: m };
    };
    return (LAYOUTS = { us: build("us"), ua: build("ua") });
  };
  // The phone keyboard's language first: Ukrainian puts ua before us.
  const preferred = (lang) => (lang === "uk" ? [layouts().ua, layouts().us] : [layouts().us, layouts().ua]);
  // The current layout when it has the character (no switching), else the first that does.
  const pick = (c, current, prefs) => {
    const cur = prefs.find((l) => l.xkb === current);
    if (cur && cur.map.has(c)) return [cur, cur.map.get(c)];
    for (const l of prefs) if (l.map.has(c)) return [l, l.map.get(c)];
    return null;
  };

  /* ═══ LineDiff: the keys that turn one line into another ═══ */
  const lineDiff = (old, oldCur, now, newCur, key, text) => {
    let p = 0;
    while (p < old.length && p < now.length && old[p] === now[p]) p++;
    let s = 0;
    while (s < old.length - p && s < now.length - p && old[old.length - 1 - s] === now[now.length - 1 - s]) s++;
    const oldEnd = old.length - s, newEnd = now.length - s;
    let at = oldCur;
    const moveTo = (to) => {
      for (let i = 0; i < at - to; i++) key("KEY_LEFT");
      for (let i = 0; i < to - at; i++) key("KEY_RIGHT");
      at = to;
    };
    if (oldEnd > p || newEnd > p) {
      moveTo(oldEnd);
      for (let i = 0; i < oldEnd - p; i++) key("KEY_BACKSPACE");
      if (newEnd > p) text(now.slice(p, newEnd));
      at = newEnd;
    }
    moveTo(newCur);
  };
  OMK.lineDiff = lineDiff;

  /* ═══ Typist: paced strokes, each with its layout ═══ */
  class Typist {
    /* o: stroke({codes, layout, char}), current() → layout, preferred() → [layouts] */
    constructor(o) { this.o = o; this.q = []; this.queued = null; this.t = null; }
    key(code, shift) { this.add(code, shift, null, null); }
    text(s) {
      const prefs = this.o.preferred();
      for (const c of s) {
        const current = this.q.length ? this.queued || this.o.current() : this.o.current();
        const p = pick(c, current, prefs);
        if (p) this.add(p[1][0], p[1][1], p[0].xkb, c);
      }
    }
    add(code, shift, layout, char) {
      this.q.push({ codes: shift ? ["KEY_LEFTSHIFT", code] : [code], layout, char });
      if (layout) this.queued = layout;
      this.pump();
    }
    clear() { this.q = []; this.queued = null; }
    pump() {
      if (this.t) return;
      const s = this.q.shift();
      if (!s) return;
      this.o.stroke(s);
      this.t = setTimeout(() => { this.t = null; this.pump(); }, STROKE_MS);
    }
  }

  /* ═══ The phone keyboard's line, mirrored on the computer (ImeCapture) ═══ */
  class Line {
    constructor(typist, shortcut, onReset) {
      this.typist = typist; this.shortcut = shortcut; this.onReset = onReset;
      this.text = ""; this.cur = 0; this.sent = ""; this.sentCur = 0;
    }
    get before() { return this.text.slice(0, this.cur); }
    set(text, cur) { this.text = text; this.cur = cur; this.sync(); }
    insert(s) { this.set(this.before + s + this.text.slice(this.cur), this.cur + s.length); }
    sync() {
      const now = this.text, cur = this.cur;
      if (now === this.sent && cur === this.sentCur) return;
      lineDiff(this.sent, this.sentCur, now, cur, (k) => this.typist.key(k), (s) => this.typist.text(s));
      this.sent = now; this.sentCur = cur;
      // A shortcut (Super from the strip + Space) typed nothing: start the line again.
      if (this.shortcut() || now.length > 400) setTimeout(() => { if (this.sent === now) this.reset(); }, 0);
    }
    backspace() {
      if (this.cur > 0) this.set(this.text.slice(0, this.cur - 1) + this.text.slice(this.cur), this.cur - 1);
      else this.typist.key("KEY_BACKSPACE"); // past the line: the text is on the computer
    }
    enter() { this.typist.key("KEY_ENTER"); setTimeout(() => this.reset(), 0); }
    reset() { this.text = ""; this.cur = 0; this.sent = ""; this.sentCur = 0; this.onReset(); }
  }

  /* ═══ Words for suggestions, autocorrect and swipe typing ═══ */
  const EN = ("the to and a i you it is of in that for on my have this we be with so are just not but at what can do if me your was all " +
    "be like now get here there about up out no yes one time know think see good great going want need come go love thanks thank " +
    "hello hi hey ok okay sure sounds cool nice wow lol home soon later tonight today tomorrow morning night movie movies watch " +
    "couch sofa phone keyboard typing type touchpad mouse screen computer desktop laptop omarchy linux arch hyprland terminal " +
    "works working work really very much too also still already almost done ready start starts started minute minutes five ten " +
    "where when how why who which will would should could let us our they them their then than from by as an or any some more " +
    "make made back way well look looks best better new old big little last first next right left long fast easy real really " +
    "pizza coffee tea dinner lunch play game games music song link send sent message call later bed late early weekend").split(" ");
  const UK = ("привіт як справи добре дякую так ні що це я ти ми ви він вона вони мене тебе там тут зараз потім завтра сьогодні " +
    "вечір ранок фільм кіно диван телефон клавіатура друкую пишу чудово супер класно гаразд будь ласка скоро вже ще трохи дуже " +
    "люблю хочу можна треба буду йду іду кава чай вечеря піца гра музика пісня посилання надішли хвилин п'ять дома вдома").split(" ");
  const TYPOS = {
    teh: "the", hte: "the", taht: "that", adn: "and", nad: "and", jsut: "just", wiht: "with", waht: "what", becuase: "because",
    keybaord: "keyboard", keyboad: "keyboard", omarhcy: "Omarchy", omarchy: "Omarchy", hyprland: "Hyprland", linux: "Linux",
    recieve: "receive", wierd: "weird", thier: "their", definately: "definitely", tommorow: "tomorrow", tonite: "tonight",
    im: "I'm", dont: "don't", cant: "can't", wont: "won't", thats: "that's", its: "it's", i: "I", u: "you", pls: "please",
    привт: "привіт", дякю: "дякую", клавятура: "клавіатура",
  };
  const words = (lang) => (lang === "uk" ? UK : EN);
  const caseLike = (model, w) => (model && model[0] !== model[0].toLowerCase() ? w[0].toUpperCase() + w.slice(1) : w);
  const isLetter = (c) => /[\p{L}']/u.test(c);

  /* ═══ The phone's own keyboard: a stand-in for whichever one you use ═══ */
  const ROWS = {
    en: ["qwertyuiop", "asdfghjkl", "zxcvbnm"],
    uk: ["йцукенгшщзх", "фівапролджє", "ячсмитьбю"],
    sym: ["1234567890", "@#$_&-+()/", "*\"':;!?"],
  };
  class PhoneKeyboard {
    /* o: line, onKey() (any key: haptics / hints), onAction(name) */
    constructor(host, o) {
      this.o = o; this.line = o.line;
      this.lang = "en"; this.page = "abc"; this.shift = "once"; this.lastSpace = 0; this.swiped = null;
      this.root = el("div", "ime");
      this.root.innerHTML = '<div class="ime-sug"></div><div class="ime-keys"></div><svg class="ime-trail" aria-hidden="true"><polyline/></svg>';
      host.appendChild(this.root);
      this.sug = $(".ime-sug", this.root);
      this.keysEl = $(".ime-keys", this.root);
      this.trail = $(".ime-trail polyline", this.root);
      this.sug.addEventListener("click", (e) => { const b = e.target.closest("button"); if (b && b.dataset.w) this.choose(b.dataset.w); });
      this.bind();
      this.render();
    }
    setLook(look) { this.root.dataset.look = look; }

    /* ── drawing ── */
    render() {
      const rows = this.page === "sym" ? ROWS.sym : ROWS[this.lang];
      const up = this.page === "abc" && this.shift !== "off";
      const k = (c) => { const v = up ? c.toUpperCase() : c; return '<button type="button" class="k l" data-c="' + esc(v) + '">' + esc(v) + "</button>"; };
      const shiftKey = this.page === "abc"
        ? '<button type="button" class="k f shift' + (this.shift === "once" ? " on" : this.shift === "caps" ? " on caps" : "") + '" data-t="shift" aria-label="Shift">⇧</button>'
        : '<button type="button" class="k f" data-t="more">=\\&lt;</button>';
      this.keysEl.innerHTML =
        '<div class="r">' + rows[0].split("").map(k).join("") + "</div>" +
        '<div class="r' + (rows[1].length < rows[0].length ? " inset" : "") + '">' + rows[1].split("").map(k).join("") + "</div>" +
        '<div class="r">' + shiftKey + rows[2].split("").map(k).join("") + '<button type="button" class="k f del" data-t="del" aria-label="Backspace">⌫</button></div>' +
        '<div class="r last"><button type="button" class="k f" data-t="page">' + (this.page === "sym" ? "ABC" : "?123") + "</button>" +
        '<button type="button" class="k l f" data-c=",">,</button>' +
        '<button type="button" class="k f" data-t="lang" aria-label="Switch language">🌐</button>' +
        '<button type="button" class="k space" data-t="space">' + (this.lang === "uk" ? "Українська" : "English") + "</button>" +
        '<button type="button" class="k l f" data-c=".">.</button>' +
        '<button type="button" class="k enter" data-t="enter" aria-label="Enter">⏎</button></div>';
      this.renderSug();
    }
    word() { const m = /[\p{L}']+$/u.exec(this.line.before); return m ? m[0] : ""; }
    suggestions() {
      if (this.swiped && this.swiped.alts.length) return this.swiped.alts;
      const w = this.word();
      if (!w) return [];
      const lw = w.toLowerCase();
      const out = [];
      if (TYPOS[lw]) out.push(TYPOS[lw]);
      for (const x of words(this.lang)) if (x.startsWith(lw) && x !== lw && out.length < 3 && !out.includes(x)) out.push(caseLike(w, x));
      if (!out.includes(w)) out.splice(TYPOS[lw] ? 1 : 0, 0, w);
      return out.slice(0, 3);
    }
    renderSug() {
      const s = this.suggestions();
      if (!s.length) {
        this.sug.innerHTML = '<span class="tools"><i>☺</i><i>⚙</i><i>✎</i><i>🎤</i></span>';
        return;
      }
      // Best in the middle, as Android keyboards do.
      const order = [s[1], s[0], s[2]];
      const auto = TYPOS[this.word().toLowerCase()];
      this.sug.innerHTML = order.map((w, i) => w == null ? "<span></span>"
        : '<button type="button" data-w="' + esc(w) + '"' + (i === 1 && (auto || this.swiped) ? ' class="best"' : "") + ">" + esc(w) + "</button>").join("");
    }

    /* ── editing ── */
    autoShift() {
      if (this.shift === "caps") return;
      this.shift = this.page === "abc" && /(^|[.!?]\s)$/.test(this.line.before) ? "once" : "off";
    }
    after() { this.autoShift(); this.render(); this.o.onKey(); }
    typeChar(c) {
      this.swiped = null;
      if (/[.,!?]/.test(c)) this.correct();
      this.line.insert(c);
      if (this.shift === "once") this.shift = "off";
      if (this.page === "sym" && c === "'") this.page = "abc";
      this.after();
    }
    // Autocorrect the word before the cursor, as space or punctuation does.
    correct() {
      const w = this.word(), fix = TYPOS[w.toLowerCase()];
      if (!w || !fix || fix === w) return false;
      const b = this.line.before;
      this.line.set(b.slice(0, b.length - w.length) + caseLike(w, fix) + this.line.text.slice(this.line.cur), this.line.cur - w.length + fix.length);
      this.o.onAction("autocorrect");
      return true;
    }
    space() {
      this.swiped = null;
      const now = performance.now(), b = this.line.before;
      if (now - this.lastSpace < 600 && /\p{L} $/u.test(b)) {
        // Double space: a full stop.
        this.line.set(b.slice(0, -1) + ". " + this.line.text.slice(this.line.cur), this.line.cur + 1);
        this.lastSpace = 0;
      } else {
        this.correct();
        this.line.insert(" ");
        this.lastSpace = now;
      }
      if (this.page === "sym") this.page = "abc";
      this.after();
    }
    choose(w) {
      const b = this.line.before;
      const cut = this.swiped ? this.swiped.word.length : this.word().length;
      // A swiped word is swapped as it is; a typed one is finished with a space.
      const tail = this.swiped ? "" : " ";
      this.line.set(b.slice(0, b.length - cut) + w + tail + this.line.text.slice(this.line.cur), this.line.cur - cut + w.length + tail.length);
      this.swiped = null;
      this.o.onAction("suggestion");
      this.after();
    }
    press(t) {
      if (t === "shift") {
        const now = performance.now();
        this.shift = this.shift === "off" ? "once" : this.shift === "once" && now - (this.shiftAt || 0) < 350 ? "caps" : "off";
        this.shiftAt = now;
        return this.render();
      }
      if (t === "del") { this.swiped = null; this.line.backspace(); return this.after(); }
      if (t === "space") return this.space();
      if (t === "enter") { this.swiped = null; this.line.enter(); this.o.onKey(); this.o.onAction("enter"); return; }
      if (t === "page" || t === "more") { this.page = this.page === "sym" ? "abc" : "sym"; return this.after(); }
      if (t === "lang") { this.setLang(this.lang === "en" ? "uk" : "en"); this.o.onAction("lang"); }
    }
    setLang(lang) { this.lang = lang; this.page = "abc"; this.swiped = null; this.after(); }
    reset() { this.swiped = null; this.page = "abc"; this.shift = "off"; this.after(); }

    /* ── swipe typing ── */
    swipe(path) {
      const letters = path.join("");
      const best = words(this.lang).map((w, rank) => {
        const sq = w.replace(/(.)\1+/g, "$1").replace(/'/g, "");
        if (sq[0] !== letters[0] || sq[sq.length - 1] !== letters[letters.length - 1]) return null;
        let i = 0;
        for (const c of letters) if (c === sq[i]) i++;
        return i === sq.length ? { w, score: sq.length * 10 - rank / 100 } : null;
      }).filter(Boolean).sort((a, b) => b.score - a.score).map((x) => x.w);
      if (!best.length) return;
      const b = this.line.before;
      const lead = b && !/\s$/.test(b) ? " " : "";
      const cap = this.shift !== "off";
      const fmt = (w) => (cap ? w[0].toUpperCase() + w.slice(1) : w);
      const word = fmt(best[0]);
      this.line.insert(lead + word);
      if (this.shift === "once") this.shift = "off";
      this.swiped = { word, alts: best.slice(0, 3).map(fmt) };
      this.o.onAction("swipe");
      this.render();
      this.o.onKey();
    }

    /* ── touch: each finger a tap on lift; a drag across letters is a swipe ── */
    bind() {
      const root = this.root;
      const fingers = new Map();
      let rects = null;
      const keyAt = (x, y) => {
        for (const r of rects) if (x >= r.l && x < r.r && y >= r.t && y < r.b) return r;
        return null;
      };
      const drawTrail = (f) => {
        const box = root.getBoundingClientRect();
        this.trail.setAttribute("points", f.pts.map((p) => (p[0] - box.left).toFixed(1) + "," + (p[1] - box.top).toFixed(1)).join(" "));
        this.trail.parentNode.classList.add("on");
      };
      root.addEventListener("pointerdown", (e) => {
        const b = e.target.closest(".ime-keys button");
        if (!b || e.button > 0) return;
        e.preventDefault();
        try { root.setPointerCapture(e.pointerId); } catch (_) {}
        rects = $$(".ime-keys button.l", root).filter((x) => /^\p{L}$/u.test(x.dataset.c)).map((x) => {
          const r = x.getBoundingClientRect();
          return { l: r.left, r: r.right, t: r.top, b: r.bottom, c: x.dataset.c.toLowerCase(), cx: r.left + r.width / 2 };
        });
        const f = { b, x: e.clientX, y: e.clientY, swiping: false, path: [], pts: [[e.clientX, e.clientY]] };
        const start = keyAt(e.clientX, e.clientY);
        if (start) f.path.push(start.c);
        fingers.set(e.pointerId, f);
        b.classList.add("down");
        if (navigator.vibrate) { try { navigator.vibrate(6); } catch (_) {} }
        if (b.dataset.t === "del") {
          // Held: repeats.
          const again = () => { this.press("del"); f.repT = setTimeout(again, 70); };
          f.rep = setTimeout(again, 420);
        }
      });
      root.addEventListener("pointermove", (e) => {
        const f = fingers.get(e.pointerId);
        if (!f || !f.b.classList.contains("l") || this.page !== "abc") return;
        f.pts.push([e.clientX, e.clientY]);
        if (!f.swiping && Math.hypot(e.clientX - f.x, e.clientY - f.y) > 14 && fingers.size === 1) {
          f.swiping = true;
          f.b.classList.remove("down");
        }
        if (!f.swiping) return;
        const k = keyAt(e.clientX, e.clientY);
        if (k && f.path[f.path.length - 1] !== k.c) f.path.push(k.c);
        drawTrail(f);
      });
      const up = (e, cancel) => {
        const f = fingers.get(e.pointerId);
        if (!f) return;
        fingers.delete(e.pointerId);
        f.b.classList.remove("down");
        clearTimeout(f.rep); clearTimeout(f.repT);
        if (cancel) return;
        if (f.swiping) {
          this.trail.parentNode.classList.remove("on");
          if (f.path.length >= 2) this.swipe(f.path);
          return;
        }
        if (f.repT) return; // the held Backspace already repeated
        if (f.b.dataset.c) this.typeChar(f.b.dataset.c);
        else if (f.b.dataset.t) this.press(f.b.dataset.t);
      };
      root.addEventListener("pointerup", (e) => up(e, false));
      root.addEventListener("pointercancel", (e) => up(e, true));
      root.addEventListener("contextmenu", (e) => e.preventDefault());
    }

    /* Scripted: tap the keys for [text], as a finger would. */
    play(text, gap, done) {
      const chars = Array.from(text);
      let retried = false;
      const next = () => {
        if (!chars.length) return done && done();
        const c = chars.shift();
        let sel;
        if (c === " ") sel = '[data-t="space"]';
        else if (c === "\n") sel = '[data-t="enter"]';
        else {
          const lower = c.toLowerCase();
          const wantUp = c !== lower;
          if (wantUp !== (this.shift !== "off") && this.page === "abc" && /\p{L}/u.test(c)) { this.press("shift"); return setTimeout(() => { chars.unshift(c); next(); }, gap); }
          sel = '.k.l[data-c="' + (this.shift !== "off" ? c.toUpperCase() : lower).replace(/"/g, '\\"') + '"]';
        }
        const b = $(sel, this.root);
        if (!b && !retried && c !== " " && c !== "\n") {
          // Not on this page: the other one has it (?123 for "!").
          this.press("page");
          chars.unshift(c);
          retried = true;
          return setTimeout(next, gap);
        }
        retried = false;
        if (b) {
          b.classList.add("down");
          setTimeout(() => {
            b.classList.remove("down");
            if (b.dataset.c) this.typeChar(b.dataset.c); else this.press(b.dataset.t);
          }, Math.min(90, gap * 0.6));
        }
        setTimeout(next, gap);
      };
      next();
    }
    /* Scripted: glide across the letters of [word]. */
    playSwipe(word, done) {
      const box = this.root.getBoundingClientRect();
      const keys = Array.from(word).map((c) => $('.k.l[data-c="' + c + '"], .k.l[data-c="' + c.toUpperCase() + '"]', this.root)).filter(Boolean);
      if (!keys.length) return done && done();
      const pts = keys.map((k) => { const r = k.getBoundingClientRect(); return [r.left + r.width / 2 - box.left, r.top + r.height / 2 - box.top]; });
      const trail = [];
      let i = 0, t = 0;
      const step = () => {
        if (i >= pts.length - 1) {
          setTimeout(() => {
            this.trail.parentNode.classList.remove("on");
            this.swipe(Array.from(word.toLowerCase()));
            done && done();
          }, 120);
          return;
        }
        t += 0.2;
        const a = pts[i], b = pts[i + 1];
        trail.push([a[0] + (b[0] - a[0]) * Math.min(t, 1), a[1] + (b[1] - a[1]) * Math.min(t, 1)]);
        this.trail.setAttribute("points", trail.map((p) => p[0].toFixed(1) + "," + p[1].toFixed(1)).join(" "));
        this.trail.parentNode.classList.add("on");
        if (t >= 1) { t = 0; i++; }
        setTimeout(step, 22);
      };
      step();
    }
  }

  /* ═══ KeyStrip: digits, F-keys, navigation and system keys, swiped sideways ═══ */
  const PAGES = [
    "1234567890".split("").map((c) => [c, "KEY_" + c]),
    Array.from({ length: 12 }, (_, i) => ["F" + (i + 1), "KEY_F" + (i + 1)]),
    [["Esc", "KEY_ESC"], ["Tab", "KEY_TAB"], ["Home", "KEY_HOME"], ["End", "KEY_END"], ["PgUp", "KEY_PAGEUP"], ["PgDn", "KEY_PAGEDOWN"],
      ["Del", "KEY_DELETE"], ["←", "KEY_LEFT"], ["↑", "KEY_UP"], ["↓", "KEY_DOWN"], ["→", "KEY_RIGHT"]],
    [["Super", "KEY_LEFTMETA", 1], ["Ctrl", "KEY_LEFTCTRL", 1], ["Alt", "KEY_LEFTALT", 1], ["Shift", "KEY_LEFTSHIFT", 1], ["PrtSc", "KEY_SYSRQ"],
      ["Menu", "KEY_COMPOSE"], ["Mute", "KEY_MUTE"], ["Vol−", "KEY_VOLUMEDOWN"], ["Vol+", "KEY_VOLUMEUP"], ["⏯", "KEY_PLAYPAUSE"]],
  ];
  class KeyStrip {
    /* o: tap(code), mod(code, down) */
    constructor(host, o) {
      this.o = o;
      this.cur = 0;
      this.root = el("div", "ks");
      this.root.innerHTML = '<div class="ks-view"><div class="ks-track">' + PAGES.map((p) => '<div class="ks-page">' +
        p.map((k) => '<button type="button" data-code="' + k[1] + '"' + (k[2] ? ' class="mod"' : "") + ">" + k[0] + "</button>").join("") + "</div>").join("") +
        '</div></div><div class="ks-dots">' + PAGES.map(() => "<i></i>").join("") + "<b></b></div>";
      host.appendChild(this.root);
      this.view = $(".ks-view", this.root);
      this.track = $(".ks-track", this.root);
      this.pill = $(".ks-dots b", this.root);
      this.bind();
      this.go(0, true);
    }
    go(n, instant) {
      this.cur = Math.max(0, Math.min(PAGES.length - 1, n));
      this.track.style.transition = instant ? "none" : "";
      this.setOffset(this.cur * this.view.clientWidth);
      this.o.onPage && this.o.onPage(this.cur);
    }
    setOffset(px) {
      const w = this.view.clientWidth || 1;
      this.track.style.transform = "translateX(" + -px + "px)";
      this.pill.style.transform = "translateX(" + (px / w) * 12 + "px)";
    }
    paint(latched) { $$("button.mod", this.root).forEach((b) => b.classList.toggle("latched", latched.has(b.dataset.code))); }
    bind() {
      const v = this.view;
      let f = null;
      v.addEventListener("pointerdown", (e) => {
        if (e.button > 0) return;
        e.preventDefault();
        const b = e.target.closest("button");
        if (f) { // a second finger: a key while the first holds a modifier
          if (b) this.press(b, e.pointerId);
          return;
        }
        try { v.setPointerCapture(e.pointerId); } catch (_) {}
        f = { id: e.pointerId, x: e.clientX, t: performance.now(), b, swiping: false, start: this.cur * v.clientWidth };
        if (b) this.press(b, e.pointerId);
      });
      v.addEventListener("pointermove", (e) => {
        if (!f || e.pointerId !== f.id) return;
        const dx = e.clientX - f.x;
        if (!f.swiping && Math.abs(dx) > 10) {
          f.swiping = true;
          if (f.b) this.cancel(f.b);
          this.track.style.transition = "none";
        }
        if (f.swiping) this.setOffset(Math.max(-30, Math.min((PAGES.length - 1) * v.clientWidth + 30, f.start - dx)));
      });
      const up = (e) => {
        if (!f || e.pointerId !== f.id) { if (this.downs && this.downs.has(e.pointerId)) this.release(e.pointerId); return; }
        const g = f; f = null;
        if (g.swiping) {
          const dx = e.clientX - g.x, fast = Math.abs(dx) / Math.max(1, performance.now() - g.t) > 0.35;
          const target = fast ? this.cur + (dx < 0 ? 1 : -1) : Math.round((g.start - dx) / v.clientWidth);
          this.go(target);
          this.o.onSwipe && this.o.onSwipe();
          return;
        }
        this.release(e.pointerId);
      };
      v.addEventListener("pointerup", up);
      v.addEventListener("pointercancel", (e) => { if (f && f.id === e.pointerId) { if (f.b) this.cancel(f.b); f = null; } });
      new ResizeObserver(() => this.go(this.cur, true)).observe(v);
    }
    press(b, pid) {
      this.downs = this.downs || new Map();
      this.downs.set(pid, b);
      b.classList.add("down");
      if (navigator.vibrate) { try { navigator.vibrate(6); } catch (_) {} }
      if (b.classList.contains("mod")) this.o.mod(b.dataset.code, true);
    }
    cancel(b) {
      b.classList.remove("down");
      for (const [pid, x] of this.downs || []) if (x === b) this.downs.delete(pid);
      if (b.classList.contains("mod")) this.o.mod(b.dataset.code, false, true);
    }
    // A key types when lifted, so a swipe that starts on one types nothing.
    release(pid) {
      const b = this.downs && this.downs.get(pid);
      if (!b) return;
      this.downs.delete(pid);
      b.classList.remove("down");
      if (b.classList.contains("mod")) this.o.mod(b.dataset.code, false);
      else this.o.tap(b.dataset.code);
    }
  }

  /* ═══ Modifiers from the strip and the touchpad: hold, or tap to keep down for the next key ═══ */
  class Mods {
    constructor(sink, paint) { this.sink = sink; this.paint = paint; this.m = new Map(); }
    get(code) { let s = this.m.get(code); if (!s) this.m.set(code, (s = { fingers: 0, latched: false, used: false, wasLatched: false, at: 0 })); return s; }
    down(code) {
      const s = this.get(code);
      if (s.fingers++ === 0) {
        s.wasLatched = s.latched; s.used = false; s.at = performance.now();
        if (!s.latched) this.sink(code, true);
      }
      this.paint();
    }
    up(code, cancel) {
      const s = this.get(code);
      if (s.fingers === 0 || --s.fingers > 0) return;
      if (cancel && s.wasLatched) return this.paint(); // a swipe across a latched key keeps it
      const tap = !cancel && !s.used && !s.wasLatched && performance.now() - s.at < 400;
      if (tap) s.latched = true;
      else { s.latched = false; this.sink(code, false); }
      this.paint();
    }
    // A key or a click went out: latched modifiers were for it; held ones now let go when lifted.
    used() {
      for (const [code, s] of this.m) {
        if (s.fingers > 0) s.used = true;
        else if (s.latched) { s.latched = false; this.sink(code, false); }
      }
      this.paint();
    }
    downSet() { const out = []; for (const [code, s] of this.m) if (s.fingers > 0 || s.latched) out.push(code); return out; }
    latchedSet() { return new Set(this.downSet()); }
  }

  /* ═══ TypedTicker: what went out, drifting right to left ═══ */
  const SYM = {
    KEY_ESC: "⎋", KEY_BACKSPACE: "⌫", KEY_TAB: "⇥", KEY_ENTER: "⏎", KEY_SPACE: "Space", KEY_UP: "↑", KEY_DOWN: "↓", KEY_LEFT: "←", KEY_RIGHT: "→",
    KEY_HOME: "Home", KEY_END: "End", KEY_PAGEUP: "PgUp", KEY_PAGEDOWN: "PgDn", KEY_DELETE: "Del", KEY_SYSRQ: "PrtSc", KEY_COMPOSE: "Menu",
    KEY_MUTE: "Mute", KEY_VOLUMEDOWN: "Vol−", KEY_VOLUMEUP: "Vol+", KEY_PLAYPAUSE: "⏯",
  };
  for (let i = 1; i <= 12; i++) SYM["KEY_F" + i] = "F" + i;
  class Ticker {
    constructor(host) {
      this.host = host;
      this.tape = el("div", "tape");
      host.appendChild(this.tape);
      this.tokens = []; this.scroll = 0; this.head = 0; this.last = 0; this.raf = 0;
      this.held = new Set(); this.next = null;
    }
    keyUp(code) { this.held.delete(code); }
    keyDown(code) {
      if (OMK.MODS.has(code)) { this.held.add(code); return; }
      const has = (m) => this.held.has("KEY_LEFT" + m) || this.held.has("KEY_RIGHT" + m);
      const really = this.next; this.next = null;
      let text, cls = "";
      if (has("CTRL") || has("ALT") || has("META")) {
        const ch = OMK.charFor(code, false, false, "us");
        const name = code === "KEY_SPACE" ? "Space" : ch && ch.trim() ? ch.toUpperCase() : SYM[code];
        if (!name) return;
        text = (has("META") ? "Super+" : "") + (has("CTRL") ? "Ctrl+" : "") + (has("ALT") ? "Alt+" : "") + (has("SHIFT") ? "Shift+" : "") + name;
        cls = "sc";
      } else if (really && really !== "\n" && really !== "\t") text = really;
      else {
        const ch = code === "KEY_SPACE" ? " " : OMK.charFor(code, has("SHIFT"), false, "us");
        if (ch) text = ch; else if (SYM[code]) { text = SYM[code]; cls = "sym"; } else return;
      }
      const W = this.host.clientWidth;
      const x = Math.max(this.head, this.scroll + W);
      const t = el("span", cls, esc(text === " " ? "\u00a0" : text));
      this.tape.appendChild(t);
      const w = t.offsetWidth;
      this.tokens.push({ t, x, w });
      t.style.transform = "translateX(" + x.toFixed(1) + "px)";
      this.head = x + w + (cls ? 6 : 0);
      if (!this.raf) { this.last = performance.now(); this.raf = requestAnimationFrame((n) => this.frame(n)); }
      this.place();
    }
    // A token sits at its tape position; the tape moves left as it scrolls.
    place() { this.tape.style.transform = "translateX(" + (-this.scroll).toFixed(1) + "px)"; }
    frame(now) {
      const dt = Math.min(0.1, (now - this.last) / 1000);
      this.last = now;
      const W = this.host.clientWidth;
      const backlog = Math.max(0, this.head - (this.scroll + W));
      this.scroll += (55 + backlog * 4) * dt;
      while (this.tokens.length && this.tokens[0].x + this.tokens[0].w < this.scroll) this.tokens.shift().t.remove();
      this.place();
      this.raf = this.tokens.length ? requestAnimationFrame((n) => this.frame(n)) : 0;
    }
  }

  /* ═══ The portrait phone ═══ */
  class PortraitPhone {
    /* o: send(code, down), pad: {onMove, onButton, onScroll, onTap}, onAction(name), host */
    constructor(root, o) {
      this.o = o;
      root.classList.add("phone", "portrait", "pp", "pad-open");
      root.innerHTML =
        '<div class="screen"><i class="pp-cam"></i>' +
        '<div class="pp-top"><button type="button" class="kb-btn" data-a="show" aria-label="Show the keyboard">⌨</button>' +
        '<span class="pp-status"><span class="kb-pill"><i>●</i><span class="pt"></span></span></span>' +
        '<button type="button" class="kb-btn" data-a="pc">⇄ PC</button><button type="button" class="kb-btn" data-a="layout">⌨ Layout</button></div>' +
        '<div class="pp-ticker"></div><div class="pp-stage"></div><div class="pp-strip"></div>' +
        '<div class="pp-kb"><div class="pp-reopen" role="button" tabindex="0"><b>⌨</b><span>Tap to open the keyboard</span></div></div>' +
        '<div class="pp-nav"><button type="button" data-a="hide" aria-label="Hide the keyboard">⌄</button><i></i><span></span></div></div>';
      this.root = root;
      this.pill = $(".kb-pill", root);
      this.status(o.host + " · 3 ms");
      this.ticker = new Ticker($(".pp-ticker", root));
      this.held = new Set();
      this.layout = null; // asked of omakeyd; null: none yet

      const send = (code, down) => {
        if (down) { if (this.held.has(code)) return; this.held.add(code); this.ticker.keyDown(code); }
        else { if (!this.held.has(code)) return; this.held.delete(code); this.ticker.keyUp(code); }
        o.send(code, down);
      };
      this.mods = new Mods(send, () => {
        const l = this.mods.latchedSet();
        this.strip && this.strip.paint(l);
        $$(".tp-col button[data-key]", root).forEach((b) => b.classList.toggle("latched", l.has(b.dataset.key)));
      });
      this.typist = new Typist({
        preferred: () => preferred(this.kb.lang),
        current: () => this.layout,
        stroke: (s) => {
          if (s.layout && s.layout !== this.layout) { this.layout = s.layout; o.onLayout && o.onLayout(s.layout); }
          this.ticker.next = s.char;
          s.codes.forEach((c) => send(c, true));
          s.codes.slice().reverse().forEach((c) => send(c, false));
          this.mods.used();
        },
      });
      const line = new Line(this.typist, () => SHORTCUT_MODS.some((m) => this.held.has(m)), () => this.kb && this.kb.reset());
      this.kb = new PhoneKeyboard($(".pp-kb", root), { line, onKey: () => {}, onAction: (a) => o.onAction(a) });
      this.line = line;

      const pad = o.pad;
      this.pad = new OMK.Touchpad($(".pp-stage", root), {
        onMove: pad.onMove, onScroll: pad.onScroll,
        onButton: (b, down) => { pad.onButton(b, down, this.mods.downSet()); if (!down) this.mods.used(); },
        onTap: (b) => { pad.onTap(b, this.mods.downSet()); this.mods.used(); },
        onMod: (code, down) => (down ? this.mods.down(code) : this.mods.up(code)),
      }, { compact: true });
      this.strip = new KeyStrip($(".pp-strip", root), {
        mod: (code, down, cancel) => (down ? this.mods.down(code) : this.mods.up(code, cancel)),
        tap: (code) => { send(code, true); send(code, false); this.mods.used(); o.onAction("strip"); },
        onSwipe: () => o.onAction("strip-swipe"),
      });

      root.addEventListener("click", (e) => {
        const a = e.target.closest("[data-a]");
        if (!a) return;
        if (a.dataset.a === "show") this.showKb(true);
        else if (a.dataset.a === "hide") this.showKb(false);
        else if (a.dataset.a === "pc") this.flash("Type on: " + o.host + " ✓");
        else if (a.dataset.a === "layout") this.flash("Portrait: your keyboard + touchpad");
      });
      $(".pp-reopen", root).addEventListener("click", () => this.showKb(true));
    }
    status(text) { this.pill.querySelector(".pt").textContent = text; }
    flash(text) { this.status(text); clearTimeout(this.ft); this.ft = setTimeout(() => this.status(this.o.host + " · 3 ms"), 2000); }
    showKb(on) {
      this.root.classList.toggle("kb-hidden", !on);
      if (on) { this.line.reset(); }
    }
  }
  OMK.PortraitPhone = PortraitPhone;

  /* ═══ The section ═══ */
  OMK.initPortrait = (root) => {
    const hintDone = (step) => $$('#pt-hints [data-step~="' + step + '"]').forEach((h) => h.classList.add("done"));
    const log = $(".pt-log", root);
    const recent = [];
    const REPLIES = ["😄 From the couch again?", "Nice. Send me the link", "Popcorn is ready 🍿", "See you in 5"];
    let r = 0;
    const desk = new OMK.Desktop($(".pt-desktop", root), {
      windows: [
        { type: "chat", title: "Messages — Olena", intro: [["them", "Movie starts in 10. Are you on the couch already?"]],
          reply: (t) => (/[а-яіїєґ]/i.test(t) ? "Ого, українською з телефона 👌" : REPLIES[r++ % REPLIES.length]) },
        { type: "term", intro: ['<span class="m"># Tap a window on the touchpad to type there.</span>', '<span class="m"># Esc, Tab, arrows and F-keys are on the strip.</span>'] },
      ],
      pointer: true, widget: false, res: [1000, 620],
      onAction: (a) => { if (a === "send") hintDone("send"); if (a === "super-space") hintDone("super"); },
    });
    desk.focus(desk.wins[0]);
    const paintLog = () => {
      log.innerHTML = '<span class="lock">INPUT layout <b>' + (phone.layout || "us") + "</b></span><span>" + (recent.length ? recent.join(" ") : "Type on the phone's keyboard: the keys it sends show here.") + "</span>";
    };
    const phone = new PortraitPhone($(".pt-phone", root), {
      host: OMK.state.hostName,
      send: (code, down) => {
        desk.key(code, down);
        if (down && !OMK.MODS.has(code)) {
          const name = code.replace(/^KEY_/, "");
          const shift = desk.held.has("KEY_LEFTSHIFT") ? "⇧" : "";
          recent.push(shift + (name === "BACKSPACE" ? "⌫" : name === "SPACE" ? "␣" : name === "ENTER" ? "⏎" : name.length === 1 ? name.toLowerCase() : name));
          if (recent.length > 18) recent.shift();
          paintLog();
        }
      },
      onLayout: (l) => {
        desk.devKl = l;
        if (l === "ua") hintDone("lang");
        paintLog();
      },
      pad: {
        onMove: (dx, dy) => desk.movePointer(dx, dy),
        onButton: (b, down, mods) => desk.button(b, down, mods),
        onScroll: (dy) => desk.scroll(dy),
        onTap: (b, mods) => desk.click(b, mods),
      },
      onAction: (a) => {
        if (a === "suggestion" || a === "autocorrect") hintDone("fix");
        if (a === "swipe") hintDone("swipe");
        if (a === "strip-swipe") hintDone("strip");
      },
    });
    paintLog();

    $$(".pt-look button", root).forEach((b) => b.addEventListener("click", () => {
      $$(".pt-look button", root).forEach((x) => x.setAttribute("aria-pressed", String(x === b)));
      phone.kb.setLook(b.dataset.look);
    }));
    phone.kb.setLook("light");

    // Scripted demos.
    let busy = false;
    const run = (fn) => { if (busy) return; busy = true; phone.showKb(true); desk.focus(desk.wins.find((w) => w.type === "chat") || desk.wins[0]); fn(() => (busy = false)); };
    const gap = () => (reduced() ? 60 : 150);
    const demos = {
      autocorrect: (done) => { if (phone.kb.lang !== "en") phone.kb.setLang("en"); phone.kb.play("im on teh couch, adn teh keybaord works\n", gap(), done); },
      swipe: (done) => {
        if (phone.kb.lang !== "en") phone.kb.setLang("en");
        const ws = ["omarchy", "works", "great"];
        const next = () => { const w = ws.shift(); if (!w) return done(); phone.kb.playSwipe(w, () => setTimeout(next, 350)); };
        next();
      },
      ukrainian: (done) => { phone.kb.setLang("uk"); phone.kb.play("привіт, дякую!\n", gap(), () => setTimeout(() => { done(); }, 200)); },
      super: (done) => {
        phone.strip.go(3);
        setTimeout(() => {
          const b = $('.ks button[data-code="KEY_LEFTMETA"]', root);
          b.classList.add("down");
          phone.mods.down("KEY_LEFTMETA");
          setTimeout(() => {
            b.classList.remove("down");
            phone.mods.up("KEY_LEFTMETA");
            setTimeout(() => phone.kb.play(" ", gap(), () => setTimeout(() => { desk.closeMenu(); done(); }, 2200)), 400);
          }, 120);
        }, 450);
      },
    };
    $$("[data-demo]", root).forEach((b) => b.addEventListener("click", () => run(demos[b.dataset.demo])));
  };
})();
