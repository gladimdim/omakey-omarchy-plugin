/* Page wiring: the hero scene, section demos, theme swatches, copy buttons. */
(function () {
  "use strict";
  const OMK = window.OMK;
  const S = OMK.state;
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const done = (scope) => (step) => $$(scope + ' [data-step~="' + step + '"]').forEach((h) => h.classList.add("done"));

  /* ── The hero: a phone typing on an Omarchy desktop ─────── */
  const heroDone = done(".try-hints");
  let touched = false;
  const log = $("#packet-log");
  let seq = 1041;
  const me = S.devices[0];
  const hero = new OMK.Desktop($("#hero-desktop"), {
    windows: [
      { type: "term", intro: ['<span class="m">Omakey is typing on this desktop. Try: omakeyd status · fastfetch · help</span>'] },
      { type: "log" },
    ],
    pointer: true,
    onLeds: (l) => heroPhone.kb.setCaps(l.caps),
    onAction: (a) => { heroDone(a); if (a === "fn") heroDone("fn"); },
  });
  const heroPhone = new OMK.Phone($("#hero-phone"), {
    layout: OMK.layout("classic-qwerty"),
    host: S.hostName,
    onKey: (code, down) => {
      touched = true;
      hero.key(code, down);
      const held = Array.from(heroPhone.kb.held.keys()).map(OMK.short);
      me.held = held.length;
      seq++;
      const ping = (2 + Math.random() * 2.2).toFixed(1);
      log.innerHTML = '<span class="lock">🔒 AES-256-GCM</span><span>INPUT <b>#' + seq + "</b></span><span>" + (down ? "↓ " : "↑ ") + OMK.short(code) +
        '</span><span class="held">held {' + held.join(", ") + "}</span><span>ACK " + ping + " ms</span>";
      if (down && !OMK.MODS.has(code) && code !== "KEY_CAPSLOCK") heroDone("type");
      OMK.emit({ type: "held" });
    },
    pad: {
      onMove: (dx, dy) => hero.movePointer(dx, dy),
      onButton: (b, down, mods) => hero.button(b, down, mods),
      onScroll: (dy) => hero.scroll(dy),
      onTap: (b) => hero.click(b),
    },
    onPadToggle: (open) => { if (open) heroDone("pad"); },
  });

  // Click the scene and your own keyboard types through the phone.
  let armed = false;
  const scene = $(".scene");
  document.addEventListener("pointerdown", (e) => { armed = scene.contains(e.target); scene.style.outline = ""; });
  const typing = (e) => e.target.closest && e.target.closest("input, textarea, select, [contenteditable]");
  document.addEventListener("keydown", (e) => {
    if (!armed || typing(e) || e.metaKey) return;
    const code = OMK.physToKey(e.code);
    if (!code) return;
    e.preventDefault();
    if (!e.repeat) heroPhone.kb.physical(code, true);
  });
  document.addEventListener("keyup", (e) => {
    if (!armed || typing(e)) return;
    const code = OMK.physToKey(e.code);
    if (code) { e.preventDefault(); heroPhone.kb.physical(code, false); }
  });
  window.addEventListener("blur", () => heroPhone.kb.releaseAll());

  // Type something by itself if nobody does, so the scene never looks idle.
  new IntersectionObserver((entries, io) => {
    if (!entries[0].isIntersecting) return;
    io.disconnect();
    setTimeout(() => { if (!touched) OMK.typeText(heroPhone.kb, "omakeyd status\n", reduced ? 30 : 110); }, reduced ? 400 : 1600);
  }, { threshold: 0.3 }).observe($("#hero-desktop"));

  /* ── A real keyboard: layouts and the lock screen ───────── */
  const real = new OMK.Desktop($("#real-desktop"), {
    windows: [{ type: "term", intro: ['<span class="m"># The phone sends key codes. The desktop layout picks the letters.</span>'] }],
    widget: false, res: [860, 470],
    onLeds: (l) => realPhone.kb.setCaps(l.caps),
  });
  const realPhone = new OMK.Phone($("#real-phone"), {
    layout: OMK.layout("classic-qwerty"), touchpad: false, compact: true, host: S.hostName,
    onKey: (code, down) => real.key(code, down),
  });
  $$("#kl-seg button").forEach((b) => b.addEventListener("click", () => {
    $$("#kl-seg button").forEach((x) => x.setAttribute("aria-pressed", String(x === b)));
    real.setKl(b.dataset.kl);
  }));
  $("#real-type").addEventListener("click", () => OMK.typeText(realPhone.kb, "ghbdsn ", 140));
  $("#real-lock").addEventListener("click", () => {
    // SUPER + CTRL + L, as three held keys.
    const kb = realPhone.kb;
    kb.emit("KEY_LEFTMETA", true); kb.emit("KEY_LEFTCTRL", true); kb.emit("KEY_L", true);
    kb.paint();
    setTimeout(() => { kb.emit("KEY_L", false); kb.emit("KEY_LEFTCTRL", false); kb.emit("KEY_LEFTMETA", false); }, 160);
  });

  /* ── Small demos ────────────────────────────────────────── */
  OMK.initTimeline($("#timeline"));
  OMK.initWire($("#wire"));
  OMK.initGallery($("#gallery"));
  OMK.initCli($("#cli-box"));
  OMK.initPortrait($("#portrait"));

  /* ── Touchpad ───────────────────────────────────────────── */
  const padDone = done("#pad-hints");
  const padDesk = new OMK.Desktop($("#pad-desktop"), {
    windows: [{ type: "files" }, { type: "doc", title: "plan.md" }],
    pointer: true, widget: false, res: [940, 560],
    onAction: padDone,
  });
  const padPhone = new OMK.Phone($("#pad-phone"), {
    layout: OMK.layout("classic-qwerty"), compact: true, host: S.hostName,
    onKey: (code, down) => padDesk.key(code, down),
    pad: {
      onMove: (dx, dy) => padDesk.movePointer(dx, dy),
      onButton: (b, down, mods) => padDesk.button(b, down, mods),
      onScroll: (dy) => padDesk.scroll(dy),
      onTap: (b) => padDesk.click(b),
    },
  });
  padPhone.setPad(true);

  /* ── Widget and pairing ─────────────────────────────────── */
  const pairDone = done("#pair-hints");
  const pairDesk = new OMK.Desktop($("#pair-desktop"), {
    windows: [{ type: "term", intro: ['<span class="m">$ # Click the keyboard icon in the bar (top right) to open the widget.</span>'] }],
    res: [1060, 720],
    onAction: (a) => pairDone(a),
  });
  pairDesk.widget.toggle(true);
  new OMK.ConnectPhone($("#pair-phone"), { onAction: pairDone });

  const swatches = $$(".swatches button");
  swatches.forEach((b) => b.addEventListener("click", () => {
    swatches.forEach((x) => x.setAttribute("aria-pressed", String(x === b)));
    OMK.setTheme(b.dataset.theme);
  }));

  // The phone's live stats in every widget: held keys, a wobbling ping.
  setInterval(() => {
    S.devices.forEach((d) => { if (d.online) d.ping = Math.max(2, Math.min(7, d.ping + Math.round(Math.random() * 2 - 1))); });
    OMK.emit({ type: "devices" });
  }, 2500);

  /* ── Copy buttons ───────────────────────────────────────── */
  $$("[data-copy]").forEach((b) => b.addEventListener("click", async () => {
    const text = document.getElementById(b.dataset.copy).textContent;
    try { await navigator.clipboard.writeText(text); b.textContent = "Copied ✓"; }
    catch (_) { b.textContent = "Select and copy"; }
    setTimeout(() => (b.textContent = "Copy"), 1800);
  }));

  /* ── Sections fade in ───────────────────────────────────── */
  const io = new IntersectionObserver((entries) => entries.forEach((e) => {
    if (e.isIntersecting) { e.target.classList.add("in"); io.unobserve(e.target); }
  }), { threshold: 0.08 });
  $$(".reveal-up").forEach((n) => io.observe(n));
})();
