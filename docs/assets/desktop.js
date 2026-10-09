/* The touchpad panel and a replica of an Omarchy desktop that reacts to
   key codes the way a real one does: the kernel sees keys, the desktop's
   layout (US or Ukrainian) turns them into text, Hyprland binds fire. */
(function () {
  "use strict";
  const OMK = (window.OMK = window.OMK || {});
  const el = (tag, cls, html) => { const e = document.createElement(tag); if (cls) e.className = cls; if (html != null) e.innerHTML = html; return e; };
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

  /* ═══ Touchpad ═══════════════════════════════════════════ */
  const PRESETS = [
    ["Laptop", 0.8], ["1080p", 0.9], ["1440p", 1.1], ["Ultrawide 1440p", 1.45], ["4K", 1.6], ["Super ultrawide", 2.1],
  ];
  const CLICK = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="6" y="3" width="12" height="18" rx="6"/><path d="M12 3v7M6 10h6"/></svg>';
  const CLICKR = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="6" y="3" width="12" height="18" rx="6"/><path d="M12 3v7M12 10h6"/></svg>';

  class Touchpad {
    /* sink: onMove(dx, dy), onButton(btn, down, mods), onScroll(dy), onTap(btn), onMod(code, down) (compact only)
       compact: portrait mode's touchpad, with plain Ctrl and Shift keys in place of Ctrl + Left and Shift + Left. */
    constructor(stage, sink, opts) {
      this.sink = sink;
      this.compact = !!(opts && opts.compact);
      this.speed = 1.1;
      this.preset = 2;
      const btns = (side) => {
        const list = this.compact
          ? [["BTN_LEFT", "Left", CLICK, []], ["BTN_RIGHT", "Right", CLICKR, []], ["KEY_LEFTCTRL", "Ctrl", "<b>⌃</b>", []], ["KEY_LEFTSHIFT", "Shift", "<b>⇧</b>", []]]
          : [["BTN_LEFT", "Left", CLICK, []], ["BTN_RIGHT", "Right", CLICKR, []], ["BTN_LEFT", "Ctrl + Left", CLICK, ["KEY_LEFTCTRL"]], ["BTN_LEFT", "Shift + Left", CLICK, ["KEY_LEFTSHIFT"]]];
        return '<div class="tp-col ' + side + '">' + list.map((b) =>
          '<button type="button" data-' + (b[0].startsWith("KEY_") ? "key" : "btn") + '="' + b[0] + '" data-mods="' + b[3].join(",") + '">' + b[2] + "<span>" + b[1] + "</span></button>").join("") + "</div>";
      };
      const hint = this.compact ? "move · tap to click<br>two fingers scroll" : "move · tap to click · hold for menu · two fingers scroll<br>scroll wheel works here too";
      const panel = el("div", "tp-panel" + (this.compact ? " compact" : ""),
        '<div class="tp-main">' + btns("l") + '<div class="tp-pad"><div class="hint">' + hint + '</div></div>' + btns("r") + "</div>" +
        '<div class="tp-strip"><span>slow</span><input type="range" min="0.3" max="3" step="0.05" aria-label="Pointer speed"><span>fast</span>' +
        '<button type="button" class="tp-chip"></button></div>');
      stage.appendChild(panel);
      this.panel = panel;
      this.pad = panel.querySelector(".tp-pad");
      const range = panel.querySelector("input");
      const chip = panel.querySelector(".tp-chip");
      const renderChip = () => {
        const v = this.speed.toFixed(2).replace(/0$/, "") + "× ▾";
        chip.textContent = this.compact ? "Speed " + v : (this.preset >= 0 ? PRESETS[this.preset][0] : "Custom") + " · " + v;
        range.value = this.speed;
      };
      range.addEventListener("input", () => { this.speed = +range.value; this.preset = -1; renderChip(); });
      chip.addEventListener("click", () => { this.preset = (this.preset + 1) % PRESETS.length; this.speed = PRESETS[this.preset][1]; renderChip(); });
      renderChip();
      panel.querySelectorAll(".tp-col button").forEach((b) => {
        const mods = b.dataset.mods ? b.dataset.mods.split(",") : [];
        const send = (down) => b.dataset.key ? this.sink.onMod && this.sink.onMod(b.dataset.key, down) : this.sink.onButton(b.dataset.btn, down, mods);
        const down = (e) => { e.preventDefault(); b.classList.add("down"); try { b.setPointerCapture(e.pointerId); } catch (_) {} send(true); };
        const up = () => { if (!b.classList.contains("down")) return; b.classList.remove("down"); send(false); };
        b.addEventListener("pointerdown", down);
        b.addEventListener("pointerup", up);
        b.addEventListener("pointercancel", up);
        b.addEventListener("contextmenu", (e) => e.preventDefault());
      });
      this.bindPad();
    }

    bindPad() {
      const pad = this.pad;
      const pts = new Map();
      let gesture = null; // { start, moved, max, x, y, timer }
      const dot = (p) => { const d = el("div", "dot"); pad.appendChild(d); return d; };
      const place = (p) => { const r = pad.getBoundingClientRect(); p.dot.style.left = p.x - r.left + "px"; p.dot.style.top = p.y - r.top + "px"; };
      pad.addEventListener("pointerdown", (e) => {
        e.preventDefault();
        try { pad.setPointerCapture(e.pointerId); } catch (_) {}
        const p = { x: e.clientX, y: e.clientY, dot: dot() };
        place(p);
        pts.set(e.pointerId, p);
        if (e.button === 2) { this.sink.onTap("BTN_RIGHT"); return; }
        if (!gesture) {
          gesture = { start: performance.now(), moved: 0, max: 1, held: false };
          gesture.timer = setTimeout(() => {
            if (gesture && gesture.moved < 8 && gesture.max === 1) { gesture.held = true; this.sink.onTap("BTN_RIGHT"); }
          }, 480);
        }
        gesture.max = Math.max(gesture.max, pts.size);
      });
      pad.addEventListener("pointermove", (e) => {
        const p = pts.get(e.pointerId);
        if (!p || !gesture) return;
        const dx = e.clientX - p.x, dy = e.clientY - p.y;
        p.x = e.clientX; p.y = e.clientY; place(p);
        gesture.moved += Math.abs(dx) + Math.abs(dy);
        if (pts.size >= 2) this.sink.onScroll(-dy / pts.size);
        else this.sink.onMove(dx * this.speed * 1.6, dy * this.speed * 1.6);
      });
      const up = (e) => {
        const p = pts.get(e.pointerId);
        if (!p) return;
        p.dot.remove();
        pts.delete(e.pointerId);
        if (pts.size || !gesture) return;
        clearTimeout(gesture.timer);
        const quick = performance.now() - gesture.start < 260 && gesture.moved < 10;
        if (quick && !gesture.held && e.button !== 2) this.sink.onTap(gesture.max === 2 ? "BTN_RIGHT" : gesture.max >= 3 ? "BTN_MIDDLE" : "BTN_LEFT");
        gesture = null;
      };
      pad.addEventListener("pointerup", up);
      pad.addEventListener("pointercancel", up);
      pad.addEventListener("contextmenu", (e) => e.preventDefault());
      pad.addEventListener("wheel", (e) => { e.preventDefault(); this.sink.onScroll(-e.deltaY / 3); }, { passive: false });
    }
  }
  OMK.Touchpad = Touchpad;

  /* ═══ Key codes → text: US and Ukrainian layouts ══════════ */
  const US = {
    KEY_1: "1!", KEY_2: "2@", KEY_3: "3#", KEY_4: "4$", KEY_5: "5%", KEY_6: "6^", KEY_7: "7&", KEY_8: "8*", KEY_9: "9(", KEY_0: "0)",
    KEY_MINUS: "-_", KEY_EQUAL: "=+", KEY_LEFTBRACE: "[{", KEY_RIGHTBRACE: "]}", KEY_BACKSLASH: "\\|", KEY_SEMICOLON: ";:",
    KEY_APOSTROPHE: "'\"", KEY_GRAVE: "`~", KEY_COMMA: ",<", KEY_DOT: ".>", KEY_SLASH: "/?", KEY_SPACE: "  ",
  };
  const UA_LETTERS = {
    Q: "й", W: "ц", E: "у", R: "к", T: "е", Y: "н", U: "г", I: "ш", O: "щ", P: "з", LEFTBRACE: "х", RIGHTBRACE: "ї",
    A: "ф", S: "і", D: "в", F: "а", G: "п", H: "р", J: "о", K: "л", L: "д", SEMICOLON: "ж", APOSTROPHE: "є",
    Z: "я", X: "ч", C: "с", V: "м", B: "и", N: "т", M: "ь", COMMA: "б", DOT: "ю", BACKSLASH: "ґ",
  };
  const UA_OTHER = { KEY_SLASH: ".,", KEY_GRAVE: "'₴", KEY_2: "2\"", KEY_3: "3№", KEY_4: "4;", KEY_6: "6:", KEY_7: "7?" };
  OMK.charFor = (code, shift, caps, kl) => {
    const name = code.replace(/^KEY_/, "");
    if (kl === "ua" && UA_LETTERS[name]) {
      const c = UA_LETTERS[name];
      return shift !== caps ? c.toUpperCase() : c;
    }
    if (/^[A-Z]$/.test(name)) return shift !== caps ? name : name.toLowerCase();
    const pair = (kl === "ua" && UA_OTHER[code]) || US[code];
    return pair ? pair[shift ? 1 : 0] : null;
  };

  /* ═══ The shared bar-widget state ═════════════════════════ */
  // Every replica on the page reads it, so pairing in one shows in all.
  const listeners = new Set();
  OMK.state = {
    theme: "tokyo",
    running: true,
    port: 47800,
    hostName: "desk",
    pairing: null, // { expires, fp }
    justPaired: null,
    devices: [
      { id: "pixel", name: "Pixel 9 Pro", online: true, transport: "wifi", addr: "192.168.1.42", loss: 0, held: 0, ping: 3, lastSeen: 0 },
      { id: "tab", name: "Galaxy Tab S9", online: false, transport: "wifi", addr: "", loss: 0, held: 0, ping: 0, lastSeen: Date.now() / 1000 - 3 * 3600 },
    ],
  };
  OMK.on = (fn) => listeners.add(fn);
  OMK.emit = (ev) => listeners.forEach((fn) => fn(ev || {}));
  OMK.setTheme = (t) => { OMK.state.theme = t; OMK.emit({ type: "theme" }); };

  /* ═══ Desktop ═════════════════════════════════════════════ */
  const W = 1280, H = 720, BAR = 30, GAP = 10;
  const MENU = [["Apps", "SUPER ALT SPACE"], ["Learn", "keybindings, docs"], ["Trigger", "capture, share"], ["Style", "theme, font"], ["Setup", "audio, wifi"],
    ["Install", "packages"], ["Remove", ""], ["Update", "omarchy"], ["About", ""], ["System", "lock, suspend"]];

  class Desktop {
    /* opts: windows: [{type, title}], interactive, pointer, widget, onLeds({caps}), onAction(name) */
    constructor(root, opts) {
      this.o = Object.assign({ windows: [{ type: "term" }], interactive: true, pointer: false, widget: true, res: [W, H], onLeds: () => {}, onAction: () => {} }, opts);
      [this.W, this.H] = this.o.res;
      this.root = root;
      root.classList.add("dt-stage");
      this.held = new Set();
      this.caps = false;
      this.kl = "us";
      this.volume = 45;
      this.bright = 80;
      this.ws = 1;
      this.workspaces = { 1: [], 2: [], 3: [], 4: [] };
      this.build();
      this.o.windows.forEach((w) => this.open(w, true));
      const term = this.wins.find((w) => w.type === "term" || w.type === "files");
      if (term) this.focus(term);
      new ResizeObserver(() => this.fit()).observe(root);
      this.fit();
      OMK.on((ev) => { if (ev.type === "theme") this.sc.dataset.theme = OMK.state.theme; this.renderBar(); if (this.widget) this.widget.render(); });
    }

    fit() {
      this.k = this.root.clientWidth / this.W || 1;
      this.sc.style.transform = "scale(" + this.k + ")";
      this.root.style.height = Math.round(this.H * this.k) + "px";
    }

    build() {
      const sc = (this.sc = el("div", "dt-screen"));
      sc.dataset.theme = OMK.state.theme;
      sc.style.width = this.W + "px"; sc.style.height = this.H + "px";
      sc.innerHTML =
        '<div class="dt-wall"></div>' +
        '<div class="dt-bar"><span class="ws"></span><span class="clock"></span><span class="right">' +
        '<span class="vol"></span><span class="kl" title="Desktop keyboard layout: click to switch"></span>' +
        '<button class="omk" type="button" aria-label="Omakey widget" style="border:0;background:transparent;cursor:pointer">' + OMK.ICON + "</button></span></div>" +
        '<div class="dt-work"></div><div class="dt-osd"><span class="what"></span><div class="track"><div class="fill"></div></div><span class="n"></span></div>';
      this.root.innerHTML = "";
      this.root.appendChild(sc);
      this.work = sc.querySelector(".dt-work");
      this.bar = sc.querySelector(".dt-bar");
      this.osd = sc.querySelector(".dt-osd");
      this.clock = sc.querySelector(".clock");
      const tick = () => { this.clock.textContent = new Date().toLocaleString([], { weekday: "long", hour: "2-digit", minute: "2-digit" }); };
      tick(); setInterval(tick, 20000);
      sc.querySelector(".kl").addEventListener("click", () => this.setKl(this.kl === "us" ? "ua" : "us"));
      if (!this.o.interactive) { sc.style.pointerEvents = "none"; this.root.setAttribute("aria-hidden", "true"); }
      if (this.o.widget) {
        this.widget = new OMK.Widget(sc, this);
        sc.querySelector(".omk").addEventListener("click", () => this.widget.toggle());
      }
      if (this.o.pointer) {
        this.ptr = { x: this.W / 2, y: this.H / 2 + 20 };
        this.pointerEl = el("div", "dt-pointer", '<svg viewBox="0 0 24 24"><path d="M4 2l15 11-6.5 1.2L16 21l-3 1.3-3.4-6.9L4 20z" fill="#fff" stroke="#111" stroke-width="1.4" stroke-linejoin="round"/></svg>');
        sc.appendChild(this.pointerEl);
        this.movePointer(0, 0);
      }
      this.renderBar();
    }

    renderBar() {
      const ws = this.bar.querySelector(".ws");
      ws.innerHTML = [1, 2, 3, 4].map((n) => (n === this.ws ? "<b>" + n + "</b>" : "<span>" + n + "</span>")).join("");
      this.bar.querySelector(".kl").textContent = this.kl === "us" ? "EN" : "UA";
      this.bar.querySelector(".vol").textContent = (this.volume === 0 ? "vol muted" : "vol " + this.volume + "%");
      const on = OMK.state.devices.some((d) => d.online) && OMK.state.running;
      this.bar.querySelector(".omk").classList.toggle("on", on);
    }

    setKl(kl) { this.kl = kl; this.renderBar(); this.o.onAction("layout-" + kl); }

    /* ── windows ── */
    get wins() { return this.workspaces[this.ws]; }

    open(spec, instant) {
      const w = Object.assign({ id: Math.random().toString(36).slice(2) }, spec);
      const node = el("div", "dt-win" + (instant ? "" : " enter"));
      w.node = node;
      if (w.type === "term") this.makeTerm(w);
      else if (w.type === "log") this.makeLog(w);
      else if (w.type === "files") this.makeFiles(w);
      else if (w.type === "doc") this.makeDoc(w);
      else if (w.type === "chat") this.makeChat(w);
      node.addEventListener("pointerdown", () => this.focus(w));
      this.work.appendChild(node);
      this.wins.push(w);
      this.focus(w);
      this.tile();
      if (!instant) requestAnimationFrame(() => requestAnimationFrame(() => node.classList.remove("enter")));
      return w;
    }

    close(w) {
      if (!w) return;
      const list = this.wins;
      const i = list.indexOf(w);
      if (i < 0) return;
      list.splice(i, 1);
      w.node.classList.add("leave");
      setTimeout(() => w.node.remove(), 260);
      // Hyprland hands focus to a neighbour; prefer a terminal so typing keeps working.
      this.focused = list.slice().reverse().find((x) => x.type === "term") || list[Math.min(i, list.length - 1)] || null;
      this.paintFocus();
      this.tile();
    }

    focus(w) { this.focused = w; this.paintFocus(); }
    paintFocus() { Object.values(this.workspaces).flat().forEach((w) => w.node.classList.toggle("focus", w === this.focused)); }

    // Hyprland's dwindle, simplified: halve the last area for each new window.
    tile() {
      const list = this.wins;
      let area = { x: GAP, y: GAP, w: this.W - 2 * GAP, h: this.H - BAR - 2 * GAP };
      list.forEach((w, i) => {
        let r = area;
        if (i < list.length - 1) {
          if (area.w >= area.h) { r = { x: area.x, y: area.y, w: (area.w - GAP) / 2, h: area.h }; area = { x: area.x + r.w + GAP, y: area.y, w: r.w, h: area.h }; }
          else { r = { x: area.x, y: area.y, w: area.w, h: (area.h - GAP) / 2 }; area = { x: area.x, y: area.y + r.h + GAP, w: area.w, h: r.h }; }
        }
        Object.assign(w.node.style, { left: r.x + "px", top: r.y + "px", width: r.w + "px", height: r.h + "px" });
      });
    }

    setWs(n) {
      if (n === this.ws) return;
      this.wins.forEach((w) => (w.node.style.display = "none"));
      this.ws = n;
      this.wins.forEach((w) => (w.node.style.display = ""));
      this.focused = this.wins[this.wins.length - 1] || null;
      this.paintFocus();
      this.renderBar();
    }

    /* ── terminal ── */
    makeTerm(w) {
      w.node.innerHTML = '<div class="dt-term"><div class="tx"></div></div>';
      w.out = w.node.querySelector(".tx");
      w.lines = [];
      w.line = ""; w.cur = 0; w.hist = []; w.hi = 0;
      (w.intro || []).forEach((l) => w.lines.push(l));
      this.renderTerm(w);
    }
    prompt() { return '<span class="d">~</span> <span class="p">❯</span> '; }
    renderTerm(w) {
      const before = esc(w.line.slice(0, w.cur)), at = w.line[w.cur], after = esc(w.line.slice(w.cur + 1));
      const cursor = at ? '<span style="background:var(--t-fg);color:var(--t-bg)">' + esc(at) + "</span>" : '<span class="dt-cursor"></span>';
      w.out.innerHTML = w.lines.slice(-60).join("\n") + (w.lines.length ? "\n" : "") + this.prompt() + before + cursor + after;
    }
    run(w, cmd) {
      const out = (s, c) => w.lines.push(c ? '<span class="' + c + '">' + esc(s) + "</span>" : esc(s));
      w.lines.push(this.prompt() + esc(cmd));
      const [c, ...args] = cmd.trim().split(/\s+/);
      const a = args.join(" ");
      if (!c) return;
      if (c === "clear") { w.lines = []; return; }
      if (c === "exit") { this.close(w); return; }
      if (c === "help") { out("Try: omakeyd status · omakeyd devices · libinput list-devices · fastfetch · echo hi · ls · clear", "m"); out("Hyprland binds: SUPER+SPACE menu · SUPER+RETURN terminal · SUPER+W close · SUPER+CTRL+L lock · SUPER+1..4", "m"); return; }
      if (c === "ls") { out("code  Documents  Downloads  Music  Pictures  Videos", "a"); return; }
      if (c === "echo") { out(a); return; }
      if (c === "whoami") { out("you"); return; }
      if (c === "date") { out(new Date().toString()); return; }
      if (c === "uname") { out("Linux desk 6.17.2-arch1-1 x86_64 GNU/Linux"); return; }
      if (c === "omakeyd" && args[0] === "status") {
        const d = OMK.state.devices.filter((x) => x.online);
        out("omakeyd 1.3.0 · running · UDP " + OMK.state.port, "k");
        d.forEach((x) => out("  ● " + x.name + "  " + "Wi-Fi " + x.addr + " · ping " + x.ping + " ms · " + this.held.size + " held"));
        if (!d.length) out("  no phones connected", "m");
        return;
      }
      if (c === "omakeyd" && args[0] === "devices") {
        OMK.state.devices.forEach((x) => out("  " + x.id.padEnd(10) + x.name.padEnd(16) + (x.online ? "connected" : "last seen " + ago(x.lastSeen))));
        return;
      }
      if (c === "omakeyd") { out("usage: omakeyd <pair|status|devices|forget|rename|config|run>", "m"); return; }
      if (c === "libinput" || c === "sudo") {
        out("Device:           Omakey Keyboard", "y"); out("Kernel:           /dev/input/event27"); out("Capabilities:     keyboard");
        out("Device:           Omakey Mouse", "y"); out("Kernel:           /dev/input/event28"); out("Capabilities:     pointer");
        return;
      }
      if (c === "fastfetch" || c === "neofetch") {
        out("you@desk", "a"); out("--------"); out("OS: Omarchy (Arch Linux)"); out("WM: Hyprland"); out("Keyboard: Omakey Keyboard (uinput)");
        out("Typing from: " + (OMK.state.devices.find((x) => x.online) || { name: "—" }).name); return;
      }
      out("bash: " + c + ": command not found", "r");
    }
    termKey(w, code, ch) {
      const ctrl = this.has("CTRL");
      if (ctrl && code === "KEY_C") { w.lines.push(this.prompt() + esc(w.line) + '<span class="m">^C</span>'); w.line = ""; w.cur = 0; }
      else if (ctrl && code === "KEY_L") { w.lines = []; }
      else if (ctrl && code === "KEY_U") { w.line = w.line.slice(w.cur); w.cur = 0; }
      else if (ctrl && code === "KEY_A") { w.cur = 0; }
      else if (ctrl && code === "KEY_E") { w.cur = w.line.length; }
      else if (ctrl) return;
      else if (code === "KEY_ENTER" || code === "KEY_KPENTER") {
        const cmd = w.line; w.line = ""; w.cur = 0;
        if (cmd.trim()) { w.hist.push(cmd); }
        w.hi = w.hist.length;
        this.run(w, cmd);
        this.o.onAction("enter");
        if (!this.wins.includes(w)) return;
      } else if (code === "KEY_BACKSPACE") { if (w.cur > 0) { w.line = w.line.slice(0, w.cur - 1) + w.line.slice(w.cur); w.cur--; } }
      else if (code === "KEY_DELETE") { w.line = w.line.slice(0, w.cur) + w.line.slice(w.cur + 1); }
      else if (code === "KEY_LEFT") w.cur = Math.max(0, w.cur - 1);
      else if (code === "KEY_RIGHT") w.cur = Math.min(w.line.length, w.cur + 1);
      else if (code === "KEY_HOME") w.cur = 0;
      else if (code === "KEY_END") w.cur = w.line.length;
      else if (code === "KEY_UP") { if (w.hi > 0) { w.hi--; w.line = w.hist[w.hi]; w.cur = w.line.length; } }
      else if (code === "KEY_DOWN") { if (w.hi < w.hist.length) { w.hi++; w.line = w.hist[w.hi] || ""; w.cur = w.line.length; } }
      else if (code === "KEY_TAB") {
        const cands = ["omakeyd status", "omakeyd devices", "fastfetch", "libinput list-devices", "help", "clear"].filter((c) => c.startsWith(w.line) && w.line);
        if (cands.length === 1) { w.line = cands[0]; w.cur = w.line.length; }
      } else if (ch) { w.line = w.line.slice(0, w.cur) + ch + w.line.slice(w.cur); w.cur += ch.length; this.o.onAction("type"); }
      this.renderTerm(w);
    }

    makeLog(w) {
      w.node.innerHTML = '<div class="dt-term" style="font-size:12.5px"><div class="tx"></div></div>';
      w.out = w.node.querySelector(".tx");
      w.lines = [
        '<span class="m">$ journalctl --user -u omakeyd -f</span>',
        '<span class="m">omakeyd[812]:</span> omakeyd 1.3.0 listening on UDP 47800, mDNS _omakey._udp',
        '<span class="m">omakeyd[812]:</span> virtual devices: "Omakey Keyboard", "Omakey Mouse"',
        '<span class="m">omakeyd[812]:</span> <span class="p">Pixel 9 Pro connected</span> over Wi-Fi 192.168.1.42 (session 7f3a)',
      ];
      w.log = (s) => { w.lines.push('<span class="m">omakeyd[812]:</span> ' + s); if (w.lines.length > 80) w.lines.splice(1, 20); w.out.innerHTML = w.lines.join("\n"); };
      w.out.innerHTML = w.lines.join("\n");
      this.logWin = w;
    }

    makeFiles(w) {
      const files = [["code", "dir"], ["Documents", "dir"], ["Music", "dir"], ["Pictures", "dir"], ["Videos", "dir"],
        ["notes.md", "doc"], ["plan.md", "doc"], ["omakey.png", "img"], ["layout.json", "doc"], ["wallpaper.jpg", "img"]];
      w.node.innerHTML = '<div class="dt-wtitle">Files — ~/</div><div class="dt-files"><div class="side"><div class="on">Home</div><div>Documents</div><div>Downloads</div><div>Pictures</div><div>Trash</div></div><div class="grid">' +
        files.map((f) => '<div class="dt-file ' + f[1] + '" data-name="' + f[0] + '"><div class="ico"></div>' + f[0] + "</div>").join("") + "</div></div>";
    }
    makeDoc(w) {
      const para = "Every touch on the phone becomes a key press on a kernel virtual keyboard, so Hyprland, games and the lock screen all see a real keyboard. ";
      w.node.innerHTML = '<div class="dt-wtitle">' + esc(w.title || "notes.md") + '</div><div class="dt-doc"><div class="inner"><h5>' + esc(w.title || "notes.md") + "</h5>" +
        Array.from({ length: 14 }, (_, i) => "<p>" + (i + 1) + ". " + para + "</p>").join("") + "</div></div>";
      w.scroll = 0;
    }

    /* A chat: what's typed goes in the message box, Enter sends it. */
    makeChat(w) {
      w.node.innerHTML = '<div class="dt-wtitle">' + esc(w.title || "Messages") + '</div><div class="dt-chat"><div class="msgs"></div><div class="in"><span class="tx"></span></div></div>';
      w.msgs = w.node.querySelector(".msgs");
      w.inp = w.node.querySelector(".in .tx");
      w.line = ""; w.cur = 0;
      (w.intro || []).forEach((m) => this.chatMsg(w, m[0], m[1]));
      this.renderChat(w);
    }
    chatMsg(w, who, text) {
      const m = el("div", "msg " + who, esc(text));
      w.msgs.appendChild(m);
      while (w.msgs.children.length > 14) w.msgs.firstChild.remove();
    }
    renderChat(w) {
      const before = esc(w.line.slice(0, w.cur)), at = w.line[w.cur], after = esc(w.line.slice(w.cur + 1));
      const cursor = at ? '<span class="sel">' + esc(at) + "</span>" : '<span class="dt-cursor"></span>';
      w.inp.innerHTML = w.line ? before + cursor + after : cursor + '<span class="ph">Message</span>';
    }
    chatKey(w, code, ch) {
      if (this.has("CTRL")) {
        if (code === "KEY_BACKSPACE") { const s = w.line.slice(0, w.cur).replace(/\S+\s*$/, ""); w.line = s + w.line.slice(w.cur); w.cur = s.length; }
        else if (code === "KEY_U") { w.line = w.line.slice(w.cur); w.cur = 0; }
        else return;
      } else if (code === "KEY_ENTER" || code === "KEY_KPENTER") {
        const text = w.line.trim();
        w.line = ""; w.cur = 0;
        if (text) {
          this.chatMsg(w, "me", text);
          this.o.onAction("send");
          if (w.reply) setTimeout(() => { const r = w.reply(text); if (r) this.chatMsg(w, "them", r); }, 900);
        }
      } else if (code === "KEY_BACKSPACE") { if (w.cur > 0) { w.line = w.line.slice(0, w.cur - 1) + w.line.slice(w.cur); w.cur--; } }
      else if (code === "KEY_DELETE") { w.line = w.line.slice(0, w.cur) + w.line.slice(w.cur + 1); }
      else if (code === "KEY_LEFT") w.cur = Math.max(0, w.cur - 1);
      else if (code === "KEY_RIGHT") w.cur = Math.min(w.line.length, w.cur + 1);
      else if (code === "KEY_HOME" || code === "KEY_UP") w.cur = 0;
      else if (code === "KEY_END" || code === "KEY_DOWN") w.cur = w.line.length;
      else if (ch) { w.line = w.line.slice(0, w.cur) + ch + w.line.slice(w.cur); w.cur += ch.length; this.o.onAction("type"); }
      this.renderChat(w);
    }

    /* ── keys ── */
    has(mod) { return this.held.has("KEY_LEFT" + mod) || this.held.has("KEY_RIGHT" + mod); }

    key(code, down) {
      if (down) this.held.add(code); else this.held.delete(code);
      if (this.logWin) this.logWin.log((down ? "↓ " : "↑ ") + code + (this.held.size ? '  <span class="m">held ' + this.held.size + "</span>" : ""));
      if (!down) return;
      if (this.lock) return this.lockKey(code);
      const sup = this.has("META"), ctrl = this.has("CTRL"), alt = this.has("ALT"), shift = this.has("SHIFT");
      // Hyprland binds.
      if (sup) {
        if (code === "KEY_SPACE") return this.menu ? this.closeMenu() : this.openMenu(), this.o.onAction("super-space");
        if (code === "KEY_ENTER") { this.closeMenu(); this.open({ type: "term" }); return this.o.onAction("super-enter"); }
        if (code === "KEY_W") { this.close(this.focused); return this.o.onAction("super-w"); }
        if (code === "KEY_L" && ctrl) { this.closeMenu(); return this.showLock(); }
        const n = /^KEY_([1-4])$/.exec(code); if (n) { this.setWs(+n[1]); return this.o.onAction("ws"); }
        if (code === "KEY_ESC") return this.toast("System menu", "Lock · Suspend · Restart · Shutdown");
        return;
      }
      if (code === "KEY_CAPSLOCK") { this.caps = !this.caps; this.o.onLeds({ caps: this.caps }); return this.o.onAction("caps"); }
      if (code === "KEY_VOLUMEUP" || code === "KEY_VOLUMEDOWN") { this.volume = Math.max(0, Math.min(100, this.volume + (code === "KEY_VOLUMEUP" ? 5 : -5))); this.showOsd("Volume", this.volume); this.renderBar(); return this.o.onAction("fn"); }
      if (code === "KEY_MUTE") { this.mutedAt = this.volume ? this.volume : this.mutedAt; this.volume = this.volume ? 0 : this.mutedAt || 45; this.showOsd(this.volume ? "Volume" : "Muted", this.volume); this.renderBar(); return this.o.onAction("fn"); }
      if (code === "KEY_BRIGHTNESSUP" || code === "KEY_BRIGHTNESSDOWN") { this.bright = Math.max(5, Math.min(100, this.bright + (code === "KEY_BRIGHTNESSUP" ? 10 : -10))); this.sc.style.filter = "brightness(" + (0.45 + this.bright / 180) + ")"; this.showOsd("Brightness", this.bright); return this.o.onAction("fn"); }
      if (code === "KEY_PLAYPAUSE" || code === "KEY_NEXTSONG" || code === "KEY_PREVIOUSSONG") {
        this.playing = code === "KEY_PLAYPAUSE" ? !this.playing : true;
        this.toast(this.playing ? "▶ Now playing" : "❚❚ Paused", code === "KEY_NEXTSONG" ? "Next track: Kyiv Nights — DakhaBrakha" : code === "KEY_PREVIOUSSONG" ? "Previous track" : "Spotify"); return this.o.onAction("fn");
      }
      if (code === "KEY_MICMUTE") { this.mic = !this.mic; return this.toast(this.mic ? "Microphone muted" : "Microphone on", ""), this.o.onAction("fn"); }
      if (code === "KEY_SYSRQ") return this.toast("Screenshot saved", "~/Pictures/screenshot.png");
      if (this.menu) return this.menuKey(code);
      const w = this.focused;
      if (!w || (w.type !== "term" && w.type !== "chat")) return;
      // devKl: the layout of Omakey's own keyboard device, when a phone asked for one (portrait mode).
      const ch = alt ? null : OMK.charFor(code, shift, this.caps, this.devKl || this.kl);
      if (w.type === "chat") return this.chatKey(w, code, ch);
      this.termKey(w, code, ch);
    }

    /* Omarchy menu */
    openMenu() {
      this.menu = { q: "", sel: 0 };
      this.menuEl = el("div", "dt-menu");
      this.sc.appendChild(this.menuEl);
      this.renderMenu();
    }
    closeMenu() { if (this.menuEl) this.menuEl.remove(); this.menu = null; this.menuEl = null; }
    renderMenu() {
      const m = this.menu;
      const items = MENU.filter((i) => i[0].toLowerCase().includes(m.q.toLowerCase()));
      m.items = items;
      m.sel = Math.min(m.sel, Math.max(0, items.length - 1));
      this.menuEl.innerHTML = '<div class="q">' + (m.q ? esc(m.q) : "<i>Go…</i>") + '<span class="dt-cursor"></span></div>' +
        items.map((it, i) => '<div class="it' + (i === m.sel ? " sel" : "") + '">' + it[0] + "<small>" + it[1] + "</small></div>").join("");
    }
    menuKey(code) {
      const m = this.menu;
      if (code === "KEY_ESC") return this.closeMenu();
      if (code === "KEY_DOWN") m.sel = Math.min(m.items.length - 1, m.sel + 1);
      else if (code === "KEY_UP") m.sel = Math.max(0, m.sel - 1);
      else if (code === "KEY_BACKSPACE") m.q = m.q.slice(0, -1);
      else if (code === "KEY_ENTER") {
        const it = m.items[m.sel];
        this.closeMenu();
        if (it && it[0] === "System") return this.showLock();
        if (it) this.toast("Omarchy menu", it[0] + " — opened from your phone");
        return;
      } else {
        const ch = OMK.charFor(code, this.has("SHIFT"), this.caps, "us");
        if (ch && ch.trim()) m.q += ch;
      }
      this.renderMenu();
    }

    /* Lock screen */
    showLock() {
      if (this.lock) return;
      this.lock = { pw: "" };
      this.lockEl = el("div", "dt-lock",
        '<div class="time"></div><div class="date"></div><div class="pw"><span>Password</span></div><div class="hint">Type any password on the phone and press Enter.</div>');
      const now = new Date();
      this.lockEl.querySelector(".time").textContent = now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      this.lockEl.querySelector(".date").textContent = now.toLocaleDateString([], { weekday: "long", month: "long", day: "numeric" });
      this.sc.appendChild(this.lockEl);
      this.o.onAction("lock");
    }
    lockKey(code) {
      const pw = this.lockEl.querySelector(".pw");
      if (code === "KEY_ENTER") {
        if (this.lock.pw.length < 1) { pw.classList.remove("bad"); void pw.offsetWidth; pw.classList.add("bad"); return; }
        pw.classList.add("good");
        setTimeout(() => { this.lockEl.remove(); this.lock = null; this.o.onAction("unlock"); }, 380);
        return;
      }
      if (code === "KEY_BACKSPACE") this.lock.pw = this.lock.pw.slice(0, -1);
      else if (code === "KEY_ESC") this.lock.pw = "";
      else { const ch = OMK.charFor(code, this.has("SHIFT"), this.caps, this.devKl || this.kl); if (ch) this.lock.pw += ch; }
      pw.innerHTML = this.lock.pw ? Array.from(this.lock.pw).map(() => "<i></i>").join("") : "<span>Password</span>";
    }

    showOsd(icon, n) {
      this.osd.querySelector(".what").textContent = icon;
      this.osd.querySelector(".fill").style.width = n + "%";
      this.osd.querySelector(".n").textContent = n + "%";
      this.osd.classList.add("on");
      clearTimeout(this.osdT);
      this.osdT = setTimeout(() => this.osd.classList.remove("on"), 1300);
    }
    toast(title, body) {
      if (this.toastEl) this.toastEl.remove();
      const t = (this.toastEl = el("div", "dt-toast", "<b>" + esc(title) + "</b><span>" + esc(body) + "</span>"));
      this.sc.appendChild(t);
      clearTimeout(this.toastT);
      this.toastT = setTimeout(() => t.remove(), 2600);
    }

    /* ── pointer (Omakey Mouse) ── */
    movePointer(dx, dy) {
      if (!this.ptr) return;
      this.ptr.x = Math.max(0, Math.min(this.W - 4, this.ptr.x + dx));
      this.ptr.y = Math.max(0, Math.min(this.H - 4, this.ptr.y + dy));
      this.pointerEl.style.transform = "translate(" + this.ptr.x + "px," + this.ptr.y + "px)";
      if (this.dragging) this.dragSelect();
      if (this.ctx && (Math.abs(dx) + Math.abs(dy)) > 0) this.hoverCtx();
    }
    hit() {
      const r = this.sc.getBoundingClientRect();
      const x = r.left + this.ptr.x * this.k + 1, y = r.top + this.ptr.y * this.k + 1;
      this.pointerEl.style.visibility = "hidden";
      const e = document.elementFromPoint(x, y);
      this.pointerEl.style.visibility = "";
      return e && this.sc.contains(e) ? e : null;
    }
    ripple() {
      const d = el("div", "dt-ripple");
      d.style.left = this.ptr.x + "px"; d.style.top = this.ptr.y + "px";
      this.sc.appendChild(d); setTimeout(() => d.remove(), 460);
    }
    click(btn, mods) {
      if (!this.ptr) return;
      this.ripple();
      const t = this.hit();
      if (this.ctx) {
        const item = t && t.closest(".dt-ctx div");
        if (item) this.toast(item.textContent, "on " + this.ctx.name);
        this.ctx.el.remove(); this.ctx = null;
        return;
      }
      if (!t) return;
      const winNode = t.closest(".dt-win");
      const w = winNode && this.wins.find((x) => x.node === winNode);
      if (w) this.focus(w);
      if (t.closest(".omk")) { this.widget && this.widget.toggle(); return; }
      const file = t.closest(".dt-file");
      if (btn === "BTN_RIGHT") return this.openCtx(file);
      if (btn === "BTN_MIDDLE") return this.toast("Middle click", "pasted the primary selection");
      if (!file) { this.sc.querySelectorAll(".dt-file.sel").forEach((f) => f.classList.remove("sel")); return; }
      const multi = mods && mods.length;
      if (mods && mods.includes("KEY_LEFTSHIFT") && this.lastFile) {
        const all = Array.from(this.sc.querySelectorAll(".dt-file"));
        const [a, b] = [all.indexOf(this.lastFile), all.indexOf(file)].sort((p, q) => p - q);
        all.forEach((f, i) => f.classList.toggle("sel", i >= a && i <= b));
      } else if (multi) file.classList.toggle("sel");
      else {
        const again = file.classList.contains("sel") && performance.now() - (this.lastClick || 0) < 600 && this.lastFile === file;
        this.sc.querySelectorAll(".dt-file.sel").forEach((f) => f.classList.remove("sel"));
        file.classList.add("sel");
        if (again && file.classList.contains("doc")) this.openDoc(file.dataset.name);
      }
      this.lastFile = file; this.lastClick = performance.now();
      this.o.onAction("click");
    }
    openDoc(name) {
      const existing = this.wins.find((x) => x.type === "doc");
      if (existing) this.close(existing);
      this.open({ type: "doc", title: name });
      this.o.onAction("open");
    }
    openCtx(file) {
      if (this.ctx) this.ctx.el.remove();
      const m = el("div", "dt-ctx", "<div>Open</div><div>Rename…</div><div>Copy</div><div>Move to Trash</div><div>Properties</div>");
      m.style.left = this.ptr.x + 4 + "px"; m.style.top = this.ptr.y + 4 + "px";
      this.sc.appendChild(m);
      this.ctx = { el: m, name: file ? file.dataset.name : "the desktop" };
      this.o.onAction("right");
    }
    hoverCtx() {
      const t = this.hit();
      const item = t && t.closest(".dt-ctx div");
      if (!item) return;
      this.ctx.el.querySelectorAll("div").forEach((d) => (d.style.background = d === item ? "color-mix(in srgb, var(--t-accent) 20%, transparent)" : "transparent"));
    }
    button(btn, down, mods) {
      if (btn !== "BTN_LEFT") { if (down) this.click(btn, mods); return; }
      if (down) { this.click(btn, mods); this.dragging = mods && mods.length ? null : { x: this.ptr.x, y: this.ptr.y }; }
      else { this.dragging = null; if (this.band) { this.band.remove(); this.band = null; } }
    }
    dragSelect() {
      const d = this.dragging;
      if (!d) return;
      if (!this.band) { this.band = el("div"); Object.assign(this.band.style, { position: "absolute", zIndex: 60, border: "1px solid var(--t-accent)", background: "color-mix(in srgb, var(--t-accent) 14%, transparent)", borderRadius: "4px", pointerEvents: "none" }); this.sc.appendChild(this.band); }
      const x = Math.min(d.x, this.ptr.x), y = Math.min(d.y, this.ptr.y), w = Math.abs(this.ptr.x - d.x), h = Math.abs(this.ptr.y - d.y);
      Object.assign(this.band.style, { left: x + "px", top: y + "px", width: w + "px", height: h + "px" });
      const sr = this.sc.getBoundingClientRect();
      this.sc.querySelectorAll(".dt-file").forEach((f) => {
        const r = f.getBoundingClientRect();
        const fx = (r.left - sr.left) / this.k, fy = (r.top - sr.top) / this.k, fw = r.width / this.k, fh = r.height / this.k;
        f.classList.toggle("sel", fx < x + w && fx + fw > x && fy < y + h && fy + fh > y);
      });
    }
    scroll(dy) {
      const t = this.ptr ? this.hit() : null;
      const node = t && t.closest(".dt-win");
      const w = (node && this.wins.find((x) => x.node === node && x.type === "doc")) || this.wins.find((x) => x.type === "doc");
      if (!w) return;
      w.scroll = Math.max(-900, Math.min(0, w.scroll + dy * 2));
      w.node.querySelector(".inner").style.transform = "translateY(" + w.scroll + "px)";
      this.o.onAction("scroll");
    }
  }
  OMK.Desktop = Desktop;

  const ago = (t) => {
    const s = Math.max(0, Date.now() / 1000 - t);
    if (s < 90) return "just now";
    if (s < 3600) return Math.round(s / 60) + " min ago";
    if (s < 86400) return Math.round(s / 3600) + " h ago";
    return Math.round(s / 86400) + " days ago";
  };
  OMK.ago = ago;
})();
