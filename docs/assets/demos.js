/* Smaller demos: touch-down timeline, the wire (lost packets, stuck keys),
   Bluetooth fallback, Bluetooth keyboard mode, layout gallery, CLI. */
(function () {
  "use strict";
  const OMK = window.OMK;
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
  const el = (tag, cls, html) => { const e = document.createElement(tag); if (cls) e.className = cls; if (html != null) e.innerHTML = html; return e; };
  const reduced = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const short = (c) => c.replace(/^KEY_/, "").replace(/^LEFT|^RIGHT(?=CTRL|SHIFT|ALT|META)/, "").replace("META", "SUPER");

  /* Typing a string by key codes (scripted demos). */
  const CHAR = { " ": "KEY_SPACE", "\n": "KEY_ENTER", "-": "KEY_MINUS", ".": "KEY_DOT", "/": "KEY_SLASH", "'": "KEY_APOSTROPHE", ",": "KEY_COMMA", "=": "KEY_EQUAL" };
  OMK.codeForChar = (ch) => CHAR[ch] || (/^[a-z]$/i.test(ch) ? "KEY_" + ch.toUpperCase() : /^[0-9]$/.test(ch) ? "KEY_" + ch : null);
  OMK.typeText = (kb, text, gap, done) => {
    let i = 0;
    const next = () => {
      if (i >= text.length) return done && done();
      const c = OMK.codeForChar(text[i++]);
      if (c) kb.tap(c, 70);
      setTimeout(next, gap || 95);
    };
    next();
  };

  /* ═══ Touch-down vs on-release timeline ══════════════════ */
  OMK.initTimeline = (root) => {
    const pad = $(".holdpad", root), chips = $(".held-chips", root);
    const tracks = { omk: $('[data-t="omk"]', root), typ: $('[data-t="typ"]', root) };
    const NAMES = ["SUPER", "SHIFT", "CTRL", "ALT", "T", "Q", "W", "E", "R", "Y"];
    const fingers = new Map();
    let t0 = 0, raf = 0;
    const SPAN = 700;
    const pos = (ms) => Math.min(96, (ms / SPAN) * 100) + "%";
    const ev = (track, cls, text, ms) => { const e = el("div", "ev " + cls, text); e.style.left = pos(ms); track.appendChild(e); return e; };
    const renderChips = () => {
      chips.innerHTML = fingers.size ? Array.from(fingers.values()).map((n) => "<span>" + n + "</span>").join("") : "<em>Nothing held.</em>";
    };
    const anim = () => {
      const ms = performance.now() - t0;
      $$(".fill", root).forEach((f) => (f.style.width = pos(ms)));
      if (fingers.size && ms < SPAN) raf = requestAnimationFrame(anim);
    };
    pad.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      try { pad.setPointerCapture(e.pointerId); } catch (_) {}
      if (!fingers.size) {
        t0 = performance.now();
        Object.values(tracks).forEach((t) => $$(".ev", t).forEach((x) => x.remove()));
        ev(tracks.omk, "dn", "↓ key down · 0 ms", 0);
        cancelAnimationFrame(raf); raf = requestAnimationFrame(anim);
      }
      const used = new Set(fingers.values());
      fingers.set(e.pointerId, NAMES.find((n) => !used.has(n)) || "KEY");
      pad.classList.add("down");
      renderChips();
    });
    const up = (e) => {
      if (!fingers.has(e.pointerId)) return;
      fingers.delete(e.pointerId);
      if (!fingers.size) {
        const ms = Math.round(performance.now() - t0);
        pad.classList.remove("down");
        ev(tracks.omk, "up", "↑ " + ms + " ms", ms > SPAN * 0.55 ? SPAN * 0.55 : ms);
        ev(tracks.typ, "late", "fires now · +" + ms + " ms", ms > SPAN * 0.6 ? SPAN * 0.6 : ms);
      }
      renderChips();
    };
    pad.addEventListener("pointerup", up);
    pad.addEventListener("pointercancel", up);
    pad.addEventListener("contextmenu", (e) => e.preventDefault());
    renderChips();
  };

  /* ═══ The wire: lost packets heal, silence releases ══════ */
  OMK.initWire = (root) => {
    const air = $(".wire", root);
    const phoneSet = $(".wire-node.l .set", root), phoneSt = $(".wire-node.l .st", root);
    const dSet = $(".wire-node.r .set", root), dSt = $(".wire-node.r .st", root);
    const lossIn = $("input[type=range]", root), lossOut = $(".loss-v", root);
    const held = new Set(); // phone side
    let queue = []; // un-ACKed events: { seq, text }
    let applied = new Set(); // daemon side
    let seq = 0, evSeq = 0, silentUntil = 0, lastHeard = performance.now(), stats = { sent: 0, lost: 0 };
    const SLOW = 6; // slow motion: 100 ms heartbeat drawn as 600 ms
    const fmt = (set) => (set.size ? "{" + Array.from(set).join(", ") + "}" : "{ }");
    const render = () => {
      phoneSet.textContent = "held " + fmt(held) + (queue.length ? "\nun-acked: " + queue.map((q) => q.text).join(" ") : "");
      dSet.textContent = "held " + fmt(applied);
      phoneSt.textContent = performance.now() < silentUntil ? "silent (pocketed, crashed…)" : stats.sent + " sent · " + stats.lost + " lost";
      phoneSt.className = "st" + (performance.now() < silentUntil ? " bad" : "");
    };
    const fly = (cls, text, fromLeft, lost, onArrive) => {
      const p = el("div", "wire-pkt " + cls, text);
      air.appendChild(p);
      const W = air.clientWidth, inset = window.matchMedia("(max-width: 600px)").matches ? 130 : 175;
      const span = W - inset * 2 - p.offsetWidth;
      const a = fromLeft ? 0 : span, b = fromLeft ? span : 0;
      const dur = (reduced() ? 300 : 1100);
      const anim = p.animate([{ transform: "translate(" + a + "px,-50%)" }, { transform: "translate(" + (lost ? (a + b) / 2 : b) + "px,-50%)" }], { duration: lost ? dur / 2 : dur, easing: "linear", fill: "forwards" });
      anim.onfinish = () => {
        if (lost) { p.textContent = "✕ lost"; p.classList.add("lost"); setTimeout(() => (p.style.opacity = 0), 200); setTimeout(() => p.remove(), 700); return; }
        p.remove(); onArrive && onArrive();
      };
    };
    const send = () => {
      if (performance.now() < silentUntil) return render();
      seq++;
      stats.sent++;
      const lost = Math.random() * 100 < +lossIn.value;
      if (lost) stats.lost++;
      const evs = queue.map((q) => q.seq);
      const label = "#" + seq + " " + fmt(held) + (queue.length ? " +" + queue.map((q) => q.text).join(" ") : "");
      const snapHeld = new Set(held), snapEvs = queue.slice();
      fly("", label, true, lost, () => {
        lastHeard = performance.now();
        // The daemon applies taps it hasn't seen, then the complete held set.
        snapEvs.forEach((q) => { if (!q.applied) { q.applied = true; dSt.textContent = "applied " + q.text; } });
        applied = snapHeld;
        dSt.className = "st ok";
        render();
        const ackLost = Math.random() * 100 < +lossIn.value / 2;
        fly("ack", "ACK #" + seq, false, ackLost, () => { queue = queue.filter((q) => !evs.includes(q.seq)); render(); });
      });
      render();
    };
    let timer = setInterval(send, 100 * SLOW);
    // The daemon's stuck-key timeout: 500 ms without a packet while keys are held.
    setInterval(() => {
      if (applied.size && performance.now() - lastHeard > 500 * SLOW) {
        applied = new Set();
        dSt.textContent = "no packet for 500 ms: released every key";
        dSt.className = "st bad";
        render();
      }
    }, 200);
    const keyEvent = (text) => { queue.push({ seq: ++evSeq, text }); send(); };
    $$("[data-w]", root).forEach((b) => b.addEventListener("click", () => {
      const a = b.dataset.w;
      if (a === "shift") {
        if (held.has("SHIFT")) { held.delete("SHIFT"); keyEvent("SHIFT↑"); b.classList.remove("on"); b.textContent = "Hold Shift"; }
        else { held.add("SHIFT"); keyEvent("SHIFT↓"); b.classList.add("on"); b.textContent = "Release Shift"; }
      } else if (a === "tap") keyEvent("A↓↑");
      else if (a === "silent") { silentUntil = performance.now() + 900 * SLOW; render(); setTimeout(render, 900 * SLOW + 50); }
    }));
    lossIn.addEventListener("input", () => (lossOut.textContent = lossIn.value + "%"));
    lossOut.textContent = lossIn.value + "%";
    // Pause when off screen.
    new IntersectionObserver((e) => {
      clearInterval(timer);
      if (e[0].isIntersecting) timer = setInterval(send, 100 * SLOW);
    }).observe(air);
    render();
  };

  /* ═══ Bluetooth fallback ═════════════════════════════════ */
  OMK.initFallback = (root) => {
    const wifi = $(".path.wifi", root), bt = $(".path.bt", root), state = $(".bt-state", root), sim = $(".bt-sim", root);
    let wifiOn = true, using = "wifi", holding = false, t = 0;
    const paint = () => {
      wifi.className = "path wifi " + (!wifiOn ? "down" : using === "wifi" ? "active" : "idle");
      bt.className = "path bt " + (using === "bt" ? "active" : "idle");
      wifi.querySelector("span").textContent = wifiOn ? "Wi-Fi · UDP 47800" : "Wi-Fi · no answer";
    };
    const hist = [];
    const say = (html) => {
      hist.push(html);
      if (hist.length > 5) hist.shift();
      state.innerHTML = hist.map((h, i) => '<div style="opacity:' + (i === hist.length - 1 ? 1 : 0.45 + i * 0.1) + '">' + (i === hist.length - 1 ? "› " : "  ") + h + "</div>").join("");
    };
    const heldTxt = () => (holding ? " · <b>SHIFT still held</b>" : "");
    const flow = () => {
      if (!sim.isConnected) return;
      const path = using === "wifi" ? wifi : bt;
      if ((using === "wifi" && !wifiOn) || reduced()) return;
      const d = el("div", "flow");
      sim.appendChild(d);
      const r = path.getBoundingClientRect(), s = sim.getBoundingClientRect();
      const y = r.top - s.top + r.height / 2 - 4;
      d.animate([{ transform: "translate(" + (r.left - s.left) + "px," + y + "px)" }, { transform: "translate(" + (r.right - s.left - 8) + "px," + y + "px)" }], { duration: 900, easing: "ease-in-out" }).onfinish = () => d.remove();
    };
    setInterval(flow, 450);
    $$("[data-b]", root).forEach((b) => b.addEventListener("click", () => {
      const a = b.dataset.b;
      if (a === "wifi") {
        wifiOn = !wifiOn;
        b.textContent = wifiOn ? "Turn Wi-Fi off" : "Turn Wi-Fi on";
        clearTimeout(t);
        if (!wifiOn) {
          say("Wi-Fi went quiet. Waiting for a WELCOME over UDP…" + heldTxt());
          paint();
          t = setTimeout(() => { using = "bt"; paint(); say("No WELCOME after 1.2 s: <b>Bluetooth RFCOMM</b> joins with the same encrypted packets." + heldTxt()); }, 1200);
        } else {
          say("Wi-Fi is back. UDP never stopped trying…" + heldTxt());
          paint();
          t = setTimeout(() => { using = "wifi"; paint(); say("WELCOME over UDP: <b>back on Wi-Fi</b>, Bluetooth stops. The new session takes over the held keys." + heldTxt()); }, 700);
        }
      } else if (a === "hold") {
        holding = !holding;
        b.classList.toggle("on", holding);
        b.textContent = holding ? "Release Shift" : "Hold Shift";
        say((holding ? "Holding SHIFT over " : "Released SHIFT over ") + (using === "wifi" ? "Wi-Fi." : "Bluetooth.") + (holding ? " Now switch the network." : ""));
      }
    }));
    paint();
    say("Connected over <b>Wi-Fi</b> · 3 ms. Hold Shift, then turn Wi-Fi off.");
  };

  /* ═══ Bluetooth keyboard mode (any computer) ═════════════ */
  OMK.initHid = (root) => {
    const screen = $(".bt-screen", root);
    let target = "Mac", text = "";
    const paint = () => { screen.innerHTML = '<span class="m">' + target + " · paired as a Bluetooth keyboard and mouse</span>\n" + text.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]) + "▌"; };
    $$(".devices button", root).forEach((b) => b.addEventListener("click", () => {
      $$(".devices button", root).forEach((x) => x.setAttribute("aria-pressed", String(x === b)));
      target = b.dataset.dev; text = ""; paint();
    }));
    let shift = false, caps = false;
    const phone = new OMK.Phone($(".phone-host", root), {
      layout: OMK.layout("classic-qwerty"), touchpad: false, compact: true, host: "Bluetooth keyboard",
      onKey: (code, down) => {
        if (code.endsWith("SHIFT")) { shift = down; return; }
        if (!down) return;
        if (code === "KEY_CAPSLOCK") { caps = !caps; phone.kb.setCaps(caps); return; }
        if (code === "KEY_BACKSPACE") text = text.slice(0, -1);
        else if (code === "KEY_ENTER") text += "\n";
        else { const c = OMK.charFor(code, shift, caps, "us"); if (c) text += c; }
        text = text.slice(-120);
        paint();
      },
    });
    phone.setStatus("ok", "Bluetooth keyboard · " + target);
    $$(".devices button", root).forEach((b) => b.addEventListener("click", () => phone.setStatus("ok", "Bluetooth keyboard · " + b.dataset.dev)));
    paint();
  };

  /* ═══ Layout gallery ═════════════════════════════════════ */
  OMK.initGallery = (root) => {
    const tabs = $(".gal-tabs", root), desc = $(".gal-meta p", root), layersBox = $(".gal-meta .layers", root), sent = $(".gal-sent", root);
    let phone;
    const show = (id) => {
      $$("button", tabs).forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.id === id)));
      const l = OMK.layout(id);
      phone.kb.setLayout(l);
      desc.textContent = l.description;
      const names = phone.kb.layerNames();
      layersBox.innerHTML = names.length ? '<span class="lbl" style="font-size:12px;color:var(--faint);align-self:center">Preview layer</span><div class="seg"><button aria-pressed="true" data-l="">base</button>' +
        names.map((n) => '<button aria-pressed="false" data-l="' + n + '">' + n + "</button>").join("") + "</div>" : "";
      $$("[data-l]", layersBox).forEach((b) => b.addEventListener("click", () => {
        $$("[data-l]", layersBox).forEach((x) => x.setAttribute("aria-pressed", String(x === b)));
        phone.kb.setPreviewLayer(b.dataset.l || null);
      }));
      sent.innerHTML = l.keys.length + " keys · " + l.width + " × " + l.height + " units" + (l.splitAt != null ? " · split: each half hugs its edge of the screen" : "");
    };
    tabs.innerHTML = OMK.layouts.map((l) => '<button type="button" data-id="' + l.id + '" aria-pressed="false">' + l.name.replace(" (crkbd)", "") + "<small>" + l.keys.length + "</small></button>").join("");
    phone = new OMK.Phone($(".phone-host", root), {
      layout: OMK.layouts[0], touchpad: false,
      onKey: (code, down) => { if (down) sent.innerHTML = "sends <b>" + code + "</b> (" + OMK.codeOf(code) + ")" + (phone.kb.layer ? " on the <b>" + phone.kb.layer + "</b> layer" : ""); },
      onLayout: (id) => show(id),
    });
    $$("button", tabs).forEach((b) => b.addEventListener("click", () => show(b.dataset.id)));
    show(OMK.layouts[0].id);
  };

  /* ═══ CLI ═════════════════════════════════════════════════ */
  OMK.initCli = (root) => {
    const out = $(".cli-out", root);
    let busy = false;
    const write = (text, cls) => { const s = el("span", cls || "", null); s.textContent = text + "\n"; out.appendChild(s); out.scrollTop = out.scrollHeight; };
    const qrText = () => {
      const svg = OMK.qr(4242, 10);
      const cells = new Set();
      (svg.match(/M(\d+) (\d+)/g) || []).forEach((m) => { const [x, y] = m.slice(1).split(" ").map(Number); cells.add(x + "," + y); });
      const lines = [];
      for (let y = -1; y < 30; y += 2) {
        let line = "  ";
        for (let x = -1; x < 30; x++) {
          const a = cells.has(x + "," + y), b = cells.has(x + "," + (y + 1));
          line += a && b ? "█" : a ? "▀" : b ? "▄" : " ";
        }
        lines.push(line);
      }
      return lines;
    };
    const S = OMK.state;
    const cmds = {
      pair: ["omakeyd pair", () => [["Scan this with the Omakey app (one phone, 5 minutes):", "a"]].concat(qrText().map((l) => [l, "q"]))
        .concat([["  Fingerprint K7QD-4M2X · the phone shows the same code", "y"], ["  Waiting for the phone… (Ctrl+C to cancel)", "m"]])],
      status: ["omakeyd status", () => [["omakeyd 1.0.0 · running · UDP " + S.port + " · Bluetooth " + S.bluetooth, "a"]]
        .concat(S.devices.map((d) => ["  " + (d.online ? "● " : "○ ") + d.name.padEnd(16) + (d.online ? (d.transport === "bluetooth" ? "Bluetooth" : "Wi-Fi " + d.addr) + " · ping " + d.ping + " ms" : "last seen " + OMK.ago(d.lastSeen || 0)), d.online ? "" : "m"]))],
      devices: ["omakeyd devices", () => [["ID          NAME             LAST SEEN", "m"]].concat(S.devices.map((d) => [d.id.padEnd(12) + d.name.padEnd(17) + (d.online ? "now" : OMK.ago(d.lastSeen || 0)), ""]))],
      config: ["omakeyd config --name desk", () => [["name: desk · port: " + S.port, ""], ["Restart to apply: systemctl --user restart omakeyd", "m"]]],
      test: ["omakeyd test-client 'omakey://pair?…' --addr 127.0.0.1 --text \"hi\"", () => [["paired as test-client · session 9c41", "a"], ["typed 2 keys · ping 0.21 ms", ""], ["(acts as a phone, for testing without one)", "m"]]],
      dry: ["omakeyd run --dry-run --port 47899", () => [["listening on UDP 47899 (dry run: prints keys instead of typing)", "a"], ["KEY_LEFTMETA ↓", ""], ["KEY_SPACE ↓", ""], ["KEY_SPACE ↑", ""], ["KEY_LEFTMETA ↑", ""]]],
    };
    write("# Click a command. Output is sample output.", "m");
    $$(".cli-cmds button", root).forEach((b) => b.addEventListener("click", () => {
      if (busy) return;
      busy = true;
      $$(".cli-cmds button", root).forEach((x) => (x.disabled = true));
      const [label, fn] = cmds[b.dataset.cmd];
      const line = el("span", "", '<span class="p">$ </span>');
      out.appendChild(line);
      let i = 0;
      const type = () => {
        line.appendChild(document.createTextNode(label.slice(i, i + 2)));
        i += 2;
        out.scrollTop = out.scrollHeight;
        if (i < label.length) return setTimeout(type, reduced() ? 0 : 16);
        line.appendChild(document.createTextNode("\n"));
        setTimeout(() => {
          fn().forEach(([t, c]) => write(t, c));
          write("", "");
          busy = false;
          $$(".cli-cmds button", root).forEach((x) => (x.disabled = false));
        }, 240);
      };
      type();
    }));
  };
  OMK.short = short;
})();
