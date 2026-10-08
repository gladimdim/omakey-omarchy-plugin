/* The Omakey phone keyboard, drawn from the real layout files (layouts.js).
   It follows the Android app: keys fire on touch-down, every pointer holds
   its own key, Fn-style layer keys switch labels and codes while held, and
   sticky keys latch modifiers. A mouse has one pointer, so with a mouse the
   modifiers and layer keys always latch: click Super, then Space. */
(function () {
  "use strict";
  const OMK = (window.OMK = window.OMK || {});
  const DATA = window.OMK_DATA;
  const NS = "http://www.w3.org/2000/svg";
  const U = 100; // SVG units per key unit

  OMK.layouts = DATA.layouts;
  OMK.layout = (id) => DATA.layouts.find((l) => l.id === id) || DATA.layouts[0];
  OMK.codeOf = (name) => (DATA.keycodes[name] ? DATA.keycodes[name][0] : 0);
  OMK.labelOf = (name) => (DATA.keycodes[name] ? DATA.keycodes[name][1] : name.replace(/^KEY_/, ""));
  OMK.short = (name) => name.replace(/^KEY_/, "").replace(/^BTN_/, "BTN_");

  const MODS = new Set(["KEY_LEFTSHIFT", "KEY_RIGHTSHIFT", "KEY_LEFTCTRL", "KEY_RIGHTCTRL", "KEY_LEFTALT", "KEY_RIGHTALT", "KEY_LEFTMETA", "KEY_RIGHTMETA"]);
  OMK.MODS = MODS;
  const PAL = {
    bg: "#1a1b26", key: "#2a2f45", mod: "#1f2335", accentKey: "#3d59a1", accent: "#7aa2f7",
    layer: "#bb9af7", fg: "#c0caf5", dim: "#565f89", onAccent: "#e0e6ff",
  };

  // Browser KeyboardEvent.code → Linux key name, for typing with a real keyboard.
  const PHYS = {
    Escape: "KEY_ESC", Backquote: "KEY_GRAVE", Minus: "KEY_MINUS", Equal: "KEY_EQUAL", Backspace: "KEY_BACKSPACE", Tab: "KEY_TAB",
    BracketLeft: "KEY_LEFTBRACE", BracketRight: "KEY_RIGHTBRACE", Backslash: "KEY_BACKSLASH", CapsLock: "KEY_CAPSLOCK",
    Semicolon: "KEY_SEMICOLON", Quote: "KEY_APOSTROPHE", Enter: "KEY_ENTER", ShiftLeft: "KEY_LEFTSHIFT", ShiftRight: "KEY_RIGHTSHIFT",
    Comma: "KEY_COMMA", Period: "KEY_DOT", Slash: "KEY_SLASH", ControlLeft: "KEY_LEFTCTRL", ControlRight: "KEY_RIGHTCTRL",
    AltLeft: "KEY_LEFTALT", AltRight: "KEY_RIGHTALT", MetaLeft: "KEY_LEFTMETA", MetaRight: "KEY_RIGHTMETA", Space: "KEY_SPACE",
    ArrowUp: "KEY_UP", ArrowDown: "KEY_DOWN", ArrowLeft: "KEY_LEFT", ArrowRight: "KEY_RIGHT", Delete: "KEY_DELETE",
    Home: "KEY_HOME", End: "KEY_END", PageUp: "KEY_PAGEUP", PageDown: "KEY_PAGEDOWN", Insert: "KEY_INSERT",
  };
  OMK.physToKey = (code) => {
    if (PHYS[code]) return PHYS[code];
    let m = /^Key([A-Z])$/.exec(code); if (m) return "KEY_" + m[1];
    m = /^Digit([0-9])$/.exec(code); if (m) return "KEY_" + m[1];
    m = /^F([0-9]{1,2})$/.exec(code); if (m) return "KEY_F" + m[1];
    return null;
  };

  const svg = (tag, attrs, parent) => {
    const e = document.createElementNS(NS, tag);
    for (const k in attrs) e.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(e);
    return e;
  };

  class Keyboard {
    /* opts: layout, onKey(name, down), onChange(), sticky, interactive */
    constructor(host, opts) {
      this.o = Object.assign({ onKey: () => {}, onChange: () => {}, sticky: false, interactive: true }, opts);
      this.host = host;
      this.el = svg("svg", { class: "kb-svg", preserveAspectRatio: "xMidYMid meet" }, host);
      this.el.setAttribute("role", "img");
      this.caps = false;
      this.previewLayer = null;
      this.sticky = !!this.o.sticky;
      this.setLayout(this.o.layout || OMK.layouts[0]);
      if (this.o.interactive) this.bindPointer();
      new ResizeObserver(() => this.fit()).observe(host);
    }

    setLayout(layout) {
      this.releaseAll();
      this.l = layout;
      this.keys = layout.keys;
      this.isLetter = this.keys.map((k) => /^KEY_[A-Z]$/.test(k.code || "") && /^\p{Lu}$/u.test(k.label));
      this.reset();
      this.el.setAttribute("aria-label", layout.name + " keyboard");
      this.fit(true);
    }

    reset() {
      this.pointers = new Map(); // pointer id → { i, code }
      this.pressCount = new Array(this.keys.length).fill(0);
      this.held = new Map(); // code → count
      this.latch = new Map(); // modifier code → 1 latched, 2 locked
      this.oneShot = null; // latched layer name
    }

    /* ── geometry ─────────────────────────────────────────── */
    fit(force) {
      const w = this.host.clientWidth, h = this.host.clientHeight;
      if (!w || !h) return;
      const l = this.l;
      const unit = Math.min(w / l.width, h / l.height);
      const slack = w - unit * l.width;
      const stretch = l.splitAt != null && slack > 1 ? slack / unit : 0;
      if (!force && stretch === this.stretch && this.drawn) return;
      this.stretch = stretch;
      const vw = (l.width + stretch) * U, vh = l.height * U;
      this.el.setAttribute("viewBox", "0 0 " + vw + " " + vh);
      this.draw();
    }

    // A rectangle at or right of splitAt moves with the gap; one crossing it stretches.
    stretched(r) {
      const s = this.l.splitAt, k = this.stretch;
      if (s == null || !k) return { x: r.x, y: r.y, w: r.w, h: r.h };
      if (r.x >= s) return { x: r.x + k, y: r.y, w: r.w, h: r.h };
      if (r.x + r.w > s) return { x: r.x, y: r.y, w: r.w + k, h: r.h };
      return { x: r.x, y: r.y, w: r.w, h: r.h };
    }

    rectsOf(k) { return [k].concat(k.parts || []).map((r) => this.stretched(r)); }

    hitTest(ux, uy) {
      for (let i = this.keys.length - 1; i >= 0; i--) {
        for (const r of this.rectsOf(this.keys[i])) {
          if (ux >= r.x && ux < r.x + r.w && uy >= r.y && uy < r.y + r.h) return i;
        }
      }
      return -1;
    }

    /* ── drawing ──────────────────────────────────────────── */
    draw() {
      const el = this.el;
      el.textContent = "";
      const gap = 0.05 * U, rad = 0.12 * U;
      this.nodes = this.keys.map((k) => {
        const g = svg("g", { class: "kb-key" }, el);
        const rects = this.rectsOf(k);
        const shapes = rects.map((p) => {
          // Parts overlap where they meet so they read as one key (KeyboardView.outline).
          const meets = (test) => rects.some((o) => o !== p && test(o));
          const e = 1e-4;
          const oy = (o) => o.y < p.y + p.h - e && p.y < o.y + o.h - e;
          const ox = (o) => o.x < p.x + p.w - e && p.x < o.x + o.w - e;
          const L = meets((o) => oy(o) && Math.abs(o.x + o.w - p.x) < e) ? -gap : gap;
          const R = meets((o) => oy(o) && Math.abs(p.x + p.w - o.x) < e) ? -gap : gap;
          const T = meets((o) => ox(o) && Math.abs(o.y + o.h - p.y) < e) ? -gap : gap;
          const B = meets((o) => ox(o) && Math.abs(p.y + p.h - o.y) < e) ? -gap : gap;
          return svg("rect", { x: p.x * U + L, y: p.y * U + T, width: Math.max(1, p.w * U - L - R), height: Math.max(1, p.h * U - T - B), rx: rad }, g);
        });
        const m = rects[0];
        const box = { x: m.x * U + gap, y: m.y * U + gap, w: m.w * U - 2 * gap, h: m.h * U - 2 * gap };
        const label = svg("text", { x: box.x + box.w / 2, y: box.y + box.h / 2, "text-anchor": "middle", "dominant-baseline": "central", "font-weight": 500 }, g);
        const sub = svg("text", { x: box.x + 0.08 * U, y: box.y + 0.22 * U, "font-size": 0.2 * U }, g);
        const fn = svg("text", { x: box.x + box.w - 0.06 * U, y: box.y + box.h - 0.08 * U, "font-size": 0.16 * U, "text-anchor": "end" }, g);
        const bar = svg("rect", { x: box.x + box.w / 2 - 0.15 * U, y: box.y + box.h - 0.07 * U, width: 0.3 * U, height: 0.04 * U, fill: PAL.bg, visibility: "hidden" }, g);
        return { g, shapes, label, sub, fn, bar, box, last: {} };
      });
      this.drawn = true;
      this.paint();
    }

    get layer() {
      for (const p of this.pointers.values()) if (this.keys[p.i].layer) return this.keys[p.i].layer;
      return this.oneShot || this.previewLayer;
    }
    get shifted() {
      return (this.held.get("KEY_LEFTSHIFT") || 0) + (this.held.get("KEY_RIGHTSHIFT") || 0) > 0;
    }

    labelFor(i, layer) {
      const k = this.keys[i];
      if (layer && k.layers && k.layers[layer]) {
        const o = k.layers[layer];
        return o.label != null ? o.label : o.code ? OMK.labelOf(o.code) : "";
      }
      if (this.isLetter[i]) return this.shifted !== this.caps ? k.label : k.label.toLowerCase();
      if (this.shifted && k.sub) return k.sub;
      return k.label;
    }

    paint() {
      if (!this.nodes) return;
      const layer = this.layer;
      this.keys.forEach((k, i) => {
        const n = this.nodes[i];
        const pressed = this.pressCount[i] > 0 || this.isLatched(i);
        const fill = pressed ? PAL.accent
          : k.layer && k.layer === layer ? PAL.accent
          : k.code === "KEY_CAPSLOCK" && this.caps ? PAL.accentKey
          : k.style === "accent" ? PAL.accentKey
          : k.style === "mod" || k.style === "fkey" ? PAL.mod : PAL.key;
        const override = layer && k.layers ? k.layers[layer] : null;
        const lc = pressed ? PAL.bg : override ? PAL.layer
          : layer && !k.layer ? PAL.dim : k.style === "accent" ? PAL.onAccent : PAL.fg;
        const text = this.labelFor(i, layer);
        const subText = !layer && k.sub ? (this.shifted ? k.label : k.sub) : "";
        const fnText = !layer && k.layers && k.layers.fn && k.layers.fn.label != null && n.box.w > 0.6 * U ? k.layers.fn.label : "";
        const key = fill + lc + text + subText + fnText + this.isLocked(i);
        if (n.last.key === key) return;
        n.last.key = key;
        n.shapes.forEach((s) => s.setAttribute("fill", fill));
        n.label.textContent = text;
        n.label.setAttribute("fill", lc);
        // Fit the label like drawFitted: big for 1–2 characters, shrink to the key.
        let fs = (Array.from(text).length <= 2 ? 0.4 : 0.24) * U;
        const est = Array.from(text).length * fs * 0.58;
        const maxW = n.box.w - 0.12 * U;
        if (est > maxW) fs = fs * maxW / est;
        n.label.setAttribute("font-size", fs.toFixed(1));
        n.sub.textContent = subText;
        n.sub.setAttribute("fill", pressed ? PAL.bg : PAL.dim);
        n.fn.textContent = fnText;
        n.fn.setAttribute("fill", pressed ? PAL.bg : PAL.layer);
        n.bar.setAttribute("visibility", this.isLocked(i) ? "visible" : "hidden");
      });
    }

    isLatched(i) {
      const k = this.keys[i];
      if (k.layer) return k.layer === this.oneShot;
      return this.latch.has(k.code);
    }
    isLocked(i) { const k = this.keys[i]; return !k.layer && this.latch.get(k.code) === 2; }

    /* ── input ────────────────────────────────────────────── */
    bindPointer() {
      const el = this.el;
      const toUnits = (e) => {
        const pt = el.createSVGPoint();
        pt.x = e.clientX; pt.y = e.clientY;
        const p = pt.matrixTransform(el.getScreenCTM().inverse());
        return [p.x / U, p.y / U];
      };
      el.addEventListener("pointerdown", (e) => {
        if (e.button > 0) return;
        e.preventDefault();
        const [x, y] = toUnits(e);
        const i = this.hitTest(x, y);
        if (i < 0) return;
        try { el.setPointerCapture(e.pointerId); } catch (_) {}
        this.down(e.pointerId, i, e.pointerType === "mouse" || this.sticky);
      });
      const up = (e) => this.up(e.pointerId);
      el.addEventListener("pointerup", up);
      el.addEventListener("pointercancel", up);
      el.addEventListener("contextmenu", (e) => e.preventDefault());
    }

    emit(code, down) {
      const n = this.held.get(code) || 0;
      if (down) { this.held.set(code, n + 1); if (n === 0) this.o.onKey(code, true); }
      else if (n > 0) { if (n === 1) { this.held.delete(code); this.o.onKey(code, false); } else this.held.set(code, n - 1); }
    }

    codeFor(i) {
      const k = this.keys[i];
      const layer = this.layer;
      if (layer && k.layers && k.layers[layer]) return k.layers[layer].code || null; // no code: off on this layer
      return k.code || null;
    }

    down(pid, i, latching) {
      if (this.pointers.has(pid)) this.up(pid);
      const k = this.keys[i];
      if (navigator.vibrate && this.o.haptics !== false) { try { navigator.vibrate(8); } catch (_) {} }
      if (k.layer) {
        if (latching) {
          this.oneShot = this.oneShot === k.layer ? null : k.layer;
          this.flash(i);
        } else {
          this.pointers.set(pid, { i, code: null });
          this.pressCount[i]++;
        }
        return this.changed();
      }
      if (latching && MODS.has(k.code)) {
        const s = this.latch.get(k.code);
        if (!s) { this.latch.set(k.code, 1); this.emit(k.code, true); }
        else if (s === 1) this.latch.set(k.code, 2);
        else { this.latch.delete(k.code); this.emit(k.code, false); }
        return this.changed();
      }
      const code = this.codeFor(i);
      this.pointers.set(pid, { i, code });
      this.pressCount[i]++;
      if (code) this.emit(code, true);
      this.changed();
    }

    up(pid) {
      const p = this.pointers.get(pid);
      if (!p) return;
      this.pointers.delete(pid);
      this.pressCount[p.i] = Math.max(0, this.pressCount[p.i] - 1);
      // The code a finger pressed is the code its lift releases, even if Fn let go first.
      if (p.code) {
        this.emit(p.code, false);
        if (!MODS.has(p.code)) this.consumeLatches();
      }
      this.changed();
    }

    // After a key, latched (not locked) modifiers and a one-shot layer let go.
    consumeLatches() {
      this.oneShot = null;
      for (const [code, s] of Array.from(this.latch)) {
        if (s === 1) { this.latch.delete(code); this.emit(code, false); }
      }
    }

    flash(i) { this.pressCount[i]++; this.paint(); setTimeout(() => { this.pressCount[i]--; this.paint(); }, 120); }

    releaseAll() {
      if (!this.held) return;
      for (const code of Array.from(this.held.keys())) { this.held.set(code, 1); this.emit(code, false); }
      this.reset && this.keys && this.reset();
    }

    changed() { this.paint(); this.o.onChange(this); }

    /* A real keyboard's key: light up every key that sends it. */
    physical(code, down) {
      const idx = [];
      this.keys.forEach((k, i) => { if (k.code === code) idx.push(i); });
      if (!idx.length) { this.emit(code, down); return this.changed(); }
      const i = idx[0];
      const pid = "phys:" + code;
      if (down) { if (!this.pointers.has(pid)) this.down(pid, i, false); }
      else this.up(pid);
    }

    /* Tap a key by code, as a finger would (used by scripted demos). */
    tap(code, ms) {
      const i = this.keys.findIndex((k) => k.code === code);
      const pid = "tap:" + code + Math.random();
      if (i < 0) { this.emit(code, true); this.emit(code, false); return; }
      this.down(pid, i, false);
      setTimeout(() => this.up(pid), ms || 90);
    }

    setCaps(on) { this.caps = !!on; this.paint(); }
    setSticky(on) {
      this.sticky = !!on;
      if (!on) { for (const [code] of Array.from(this.latch)) this.emit(code, false); this.latch.clear(); this.oneShot = null; this.changed(); }
    }
    setPreviewLayer(name) { this.previewLayer = name; this.paint(); }
    layerNames() {
      const s = new Set();
      this.keys.forEach((k) => { if (k.layer) s.add(k.layer); if (k.layers) Object.keys(k.layers).forEach((n) => s.add(n)); });
      return Array.from(s);
    }
  }
  OMK.Keyboard = Keyboard;

  /* ── A phone around a keyboard, with the app's top bar ──────────── */
  OMK.ICON = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11.5,4 H7 C4.79,4 3,5.79 3,8 V17 C3,19.21 4.79,21 7,21 H16 C18.21,21 20,19.21 20,17 V13"/><path d="M15,11.5 V14.5 H8 M10.5,12 L8,14.5 L10.5,17"/><path d="M16,3 C19.31,3 22,5.69 22,9 M16,7 C17.1,7 18,7.9 18,9"/></svg>';

  class Phone {
    /* opts: layout, host ("desk"), onKey, onPad (touchpad sink), touchpad, layouts (switcher list), compact */
    constructor(root, opts) {
      this.o = Object.assign({ host: "desk", touchpad: true, layouts: null, onKey: () => {}, onLayout: () => {} }, opts);
      root.classList.add("phone");
      if (this.o.compact) root.classList.add("compact");
      root.innerHTML =
        '<div class="cam"></div><div class="screen">' +
        '<div class="kb-top"><span class="kb-pill"><i>●</i><span class="pt"></span></span><span class="kb-spacer"></span>' +
        (this.o.touchpad ? '<button class="kb-handle" type="button" aria-label="Touchpad: tap to open">⌄  touchpad</button>' : "") +
        '<button class="kb-btn opt sticky" type="button" aria-label="Sticky keys">⇧</button>' +
        '<button class="kb-btn opt" type="button" data-act="pc" aria-label="Switch computer">⇄</button>' +
        '<button class="kb-btn layout" type="button" aria-label="Switch layout">⌨</button></div>' +
        '<div class="kb-stage"></div></div>';
      this.root = root;
      this.stage = root.querySelector(".kb-stage");
      this.pill = root.querySelector(".kb-pill");
      this.kb = new Keyboard(this.stage, {
        layout: this.o.layout,
        onKey: (code, down) => this.o.onKey(code, down),
        onChange: () => this.o.onChange && this.o.onChange(this.kb),
      });
      const st = root.querySelector(".sticky");
      st.addEventListener("click", () => {
        this.kb.setSticky(!this.kb.sticky);
        st.classList.toggle("on", this.kb.sticky);
      });
      const lb = root.querySelector(".layout");
      const list = this.o.layouts || OMK.layouts.map((l) => l.id);
      lb.addEventListener("click", () => {
        const at = list.indexOf(this.kb.l.id);
        this.setLayout(list[(at + 1) % list.length]);
      });
      lb.title = "Next layout";
      root.querySelector('[data-act="pc"]').addEventListener("click", () => {
        // The real app lists paired computers here; one tap switches which one you type on.
        this.setStatus("ok", "Type on: " + this.o.host + " ✓ · studio-pc (nearby)");
        clearTimeout(this.pcT);
        this.pcT = setTimeout(() => this.setStatus("ok", this.o.host + " · 3 ms"), 2200);
      });
      this.setStatus("ok", this.o.host + " · 3 ms");
      if (this.o.touchpad) {
        this.pad = new OMK.Touchpad(this.stage, this.o.pad || {});
        const h = root.querySelector(".kb-handle");
        h.addEventListener("click", () => this.setPad(!this.padOpen));
      }
    }
    setLayout(id) {
      this.kb.setLayout(OMK.layout(id));
      this.o.onLayout(id);
    }
    setPad(open) {
      this.padOpen = open;
      this.root.classList.toggle("pad-open", open);
      const h = this.root.querySelector(".kb-handle");
      if (h) h.textContent = open ? "⌃  keyboard" : "⌄  touchpad";
      if (open) this.kb.releaseAll(), this.kb.paint();
      this.o.onPadToggle && this.o.onPadToggle(open);
    }
    setStatus(kind, text) {
      this.pill.className = "kb-pill" + (kind === "ok" ? "" : kind === "warn" ? " warn" : " err");
      this.pill.querySelector(".pt").textContent = text;
    }
  }
  OMK.Phone = Phone;
})();
