/* The Omakey bar widget's panel, and the phone's connect screen that pairs
   with it. Both read and write OMK.state, so every replica stays in step. */
(function () {
  "use strict";
  const OMK = window.OMK;
  const S = OMK.state;
  const el = (tag, cls, html) => { const e = document.createElement(tag); if (cls) e.className = cls; if (html != null) e.innerHTML = html; return e; };
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const ICON = {
    pencil: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>',
    unlink: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 17H7A5 5 0 0 1 7 7h2M15 7h2a5 5 0 0 1 4 8M8 12h3M3 3l18 18"/></svg>',
    check: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12l5 5L20 7"/></svg>',
  };

  /* A demo QR code: real finder patterns, pseudo-random data. Not scannable on purpose. */
  OMK.qr = (seed, size) => {
    const n = 29;
    let s = seed;
    const rnd = () => ((s = (s * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
    const cells = [];
    const finder = (x, y) => { for (let i = 0; i < 7; i++) for (let j = 0; j < 7; j++) { const edge = i === 0 || j === 0 || i === 6 || j === 6; const core = i >= 2 && i <= 4 && j >= 2 && j <= 4; if (edge || core) cells.push([x + i, y + j]); } };
    const inFinder = (x, y) => (x < 8 && y < 8) || (x > n - 9 && y < 8) || (x < 8 && y > n - 9);
    finder(0, 0); finder(n - 7, 0); finder(0, n - 7);
    for (let i = 8; i < n - 8; i++) { if (i % 2 === 0) { cells.push([i, 6]); cells.push([6, i]); } }
    for (let x = 0; x < n; x++) for (let y = 0; y < n; y++) {
      if (inFinder(x, y) || x === 6 || y === 6) continue;
      if (rnd() > 0.52) cells.push([x, y]);
    }
    return '<svg viewBox="-1 -1 ' + (n + 2) + " " + (n + 2) + '" width="' + (size || 180) + '" shape-rendering="crispEdges"><path fill="currentColor" d="' +
      cells.map((c) => "M" + c[0] + " " + c[1] + "h1v1h-1z").join("") + '"/></svg>';
  };

  const fpOf = () => {
    const A = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    const r = () => A[Math.floor(Math.random() * A.length)];
    return r() + r() + r() + r() + "-" + r() + r() + r() + r();
  };

  OMK.startPairing = () => {
    S.pairing = { expires: Date.now() / 1000 + 300, fp: fpOf(), seed: Math.floor(Math.random() * 1e6) };
    S.justPaired = null;
    S.pairedAtStart = S.devices.length;
    OMK.emit({ type: "pairing" });
  };
  OMK.cancelPairing = () => { S.pairing = null; OMK.emit({ type: "pairing" }); };
  OMK.completePairing = (name) => {
    if (!S.pairing) return false;
    S.pairing = null;
    const id = "phone" + Math.random().toString(36).slice(2, 6);
    S.devices.unshift({ id, name, online: true, transport: "wifi", addr: "192.168.1." + (50 + Math.floor(Math.random() * 40)), loss: 0, held: 0, ping: 4, lastSeen: 0 });
    S.justPaired = name;
    OMK.emit({ type: "paired", id });
    return true;
  };
  setInterval(() => {
    if (S.pairing) {
      if (S.pairing.expires < Date.now() / 1000) S.pairing = null;
      OMK.emit({ type: "tick" });
    }
  }, 1000);

  /* ═══ The panel ═══════════════════════════════════════════ */
  class Widget {
    constructor(screen, desktop) {
      this.d = desktop;
      this.p = el("div", "wg-panel hidden");
      screen.appendChild(this.p);
      this.ui = { renaming: "", confirm: "", settings: false, copied: false };
      this.p.addEventListener("click", (e) => this.onClick(e));
      this.p.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && e.target.matches("input.rn")) this.rename(e.target.value);
        if (e.key === "Escape") { this.ui.renaming = ""; this.render(); }
        e.stopPropagation();
      });
      OMK.on(() => this.render());
      this.render();
    }
    toggle(open) {
      this.open = open == null ? !this.open : open;
      this.p.classList.toggle("hidden", !this.open);
      if (this.open) this.render();
      this.d.o.onAction("widget");
    }
    status() {
      if (!S.running) return S.starting ? "Starting…" : "The service is off. Start it to type from your phone.";
      if (S.pairing) return "Open Omakey on your phone, tap Scan QR code, and point it here.";
      if (S.justPaired) return "Paired with " + S.justPaired + ". Start typing.";
      const c = S.devices.filter((d) => d.online).length;
      if (c === 1) return "1 phone is connected.";
      if (c > 1) return c + " phones are connected.";
      if (!S.devices.length) return "Pair your phone to use it as a keyboard.";
      return "Ready. Open Omakey on your phone to connect.";
    }
    render() {
      if (!this.open && this.rendered) return;
      this.rendered = true;
      const ui = this.ui;
      const connected = S.running && S.devices.some((d) => d.online);
      let h = '<div class="wg-head"><div class="ico">' + OMK.ICON + '</div><div><b>Omakey <i class="' + (connected ? "on" : "") + '"></i></b><p>' + esc(this.status()) + "</p></div></div>";
      if (!S.running) {
        h += '<button class="wg-btn" data-a="start">' + (S.starting ? "Starting…" : "Start Omakey") + "</button>";
      } else if (S.pairing) {
        const left = Math.max(0, Math.round(S.pairing.expires - Date.now() / 1000));
        h += '<div class="wg-qr">' + OMK.qr(S.pairing.seed, 188) + "</div>" +
          '<div class="wg-sub">One phone · expires in ' + Math.floor(left / 60) + ":" + ("0" + (left % 60)).slice(-2) + "</div>" +
          '<div class="wg-sub">Fingerprint <b style="color:var(--t-accent)">' + S.pairing.fp + "</b> · the phone shows the same code</div>" +
          '<div class="wg-row2"><button class="wg-btn sec" data-a="copy">' + (ui.copied ? "Copied" : "Copy link") + '</button><button class="wg-btn sec" data-a="cancel">Cancel</button></div>';
      } else {
        h += '<button class="wg-btn" data-a="pair">' + (S.devices.length ? "Pair another phone" : "Pair a phone") + "</button>";
      }
      if (S.devices.length) {
        h += '<div class="wg-sec">PHONES</div>';
        S.devices.forEach((d) => {
          const live = S.running && d.online;
          const confirming = ui.confirm === d.id, renaming = ui.renaming === d.id;
          const info = confirming ? '<span class="urgent">Forget this phone? It will need a new QR code.</span>'
            : renaming ? "<span>Enter to save, Esc to cancel</span>"
            : "<span>" + (live ? "Connected · " + "Wi-Fi " + d.addr + " · " + d.ping + " ms" +
              (d.loss >= 1 ? " · " + Math.round(d.loss) + "% lost" : "") + (d.held > 0 ? " · " + d.held + " held" : "")
              : "Last seen " + OMK.ago(d.lastSeen || Date.now() / 1000 - 60)) + "</span>";
          const acts = confirming ? '<button class="txt urgent" data-a="forget" data-id="' + d.id + '">Forget</button><button class="txt" data-a="keep">Keep</button>'
            : renaming ? '<button data-a="save" title="Save">' + ICON.check + "</button>"
            : '<button data-a="rename" data-id="' + d.id + '" title="Rename">' + ICON.pencil + '</button><button data-a="confirm" data-id="' + d.id + '" title="Forget this phone">' + ICON.unlink + "</button>";
          h += '<div class="wg-dev"><span class="dot' + (live ? " on" : "") + '"></span><div class="info">' +
            (renaming ? '<input class="rn" maxlength="64" value="' + esc(d.name) + '">' : "<b>" + esc(d.name) + "</b>") + info + '</div><div class="acts">' + acts + "</div></div>";
        });
      }
      h += '<div class="wg-sec">SERVICE</div><div class="wg-svc">' +
        (S.running ? '<button data-a="stop">Stop</button><button data-a="restart">Restart</button>' : '<button data-a="start">Start</button>') +
        '<button data-a="logs">Logs</button></div>';
      h += '<div class="wg-sec">SETTINGS<span data-a="settings">' + (ui.settings ? "▴" : "▾") + "</span></div>";
      if (ui.settings) {
        h += '<div class="wg-set"><label>Name your phone shows<input class="nm" value="' + esc(S.hostName) + '"></label>' +
          '<label>UDP port (phones need a new QR code after a change)<input class="pt" value="' + S.port + '"></label>' +
          '<button class="wg-btn sec" data-a="savecfg" style="margin-top:2px">Save and restart</button></div>';
      }
      h += '<div class="wg-foot"><span>' + esc(S.hostName) + " · UDP " + S.port + "</span></div>";
      const focusRename = ui.renaming && !this.p.querySelector("input.rn");
      if (this.p.contains(document.activeElement) && document.activeElement.matches("input")) return; // don't clobber typing
      this.p.innerHTML = h;
      if (focusRename) { const i = this.p.querySelector("input.rn"); if (i) { i.focus(); i.select(); } }
    }
    rename(v) {
      const d = S.devices.find((x) => x.id === this.ui.renaming);
      if (d && v.trim()) d.name = v.trim().slice(0, 64);
      this.ui.renaming = "";
      this.p.querySelector("input.rn") && this.p.querySelector("input.rn").blur();
      OMK.emit({ type: "devices" });
    }
    onClick(e) {
      const b = e.target.closest("[data-a]");
      if (!b) return;
      e.stopPropagation();
      const a = b.dataset.a, ui = this.ui;
      if (a === "pair") { OMK.startPairing(); this.d.o.onAction("pair"); return; }
      if (a === "cancel") return OMK.cancelPairing();
      if (a === "copy") { ui.copied = true; S.clipboard = true; this.render(); setTimeout(() => { ui.copied = false; this.render(); }, 1500); return; }
      if (a === "rename") { ui.confirm = ""; ui.renaming = b.dataset.id; }
      else if (a === "save") return this.rename(this.p.querySelector("input.rn").value);
      else if (a === "confirm") { ui.renaming = ""; ui.confirm = b.dataset.id; }
      else if (a === "keep") ui.confirm = "";
      else if (a === "forget") { S.devices = S.devices.filter((d) => d.id !== b.dataset.id); ui.confirm = ""; OMK.emit({ type: "devices" }); return; }
      else if (a === "stop") { S.running = false; OMK.emit({ type: "service" }); return; }
      else if (a === "start" || a === "restart") {
        S.running = false; S.starting = true; OMK.emit({ type: "service" });
        setTimeout(() => { S.starting = false; S.running = true; OMK.emit({ type: "service" }); }, 900);
        return;
      } else if (a === "logs") { this.d.toast("journalctl --user -u omakeyd -f", "opened in a terminal"); }
      else if (a === "settings") ui.settings = !ui.settings;
      else if (a === "savecfg") {
        const nm = this.p.querySelector("input.nm").value.trim(), pt = parseInt(this.p.querySelector("input.pt").value, 10);
        if (nm) S.hostName = nm.slice(0, 40);
        if (pt > 1023 && pt < 65536) S.port = pt;
        document.activeElement && document.activeElement.blur();
        S.running = false; S.starting = true; OMK.emit({ type: "service" });
        setTimeout(() => { S.starting = false; S.running = true; OMK.emit({ type: "service" }); }, 900);
        return;
      }
      this.render();
    }
  }
  OMK.Widget = Widget;

  /* ═══ The phone's connect screen (portrait) ══════════════ */
  class ConnectPhone {
    constructor(root, opts) {
      this.o = Object.assign({ name: "Galaxy Z Fold7", onAction: () => {} }, opts);
      root.classList.add("phone", "portrait");
      root.innerHTML = '<div class="screen"><div class="cn"></div></div>';
      this.cn = root.querySelector(".cn");
      this.cn.addEventListener("click", (e) => this.onClick(e));
      OMK.on((ev) => { if (ev.type !== "tick" && ev.type !== "theme") this.render(); if (ev.type === "pairing" && this.scanning && S.pairing) this.found(); });
      this.render();
    }
    render() {
      if (this.scanning || this.dialog) return;
      const paired = S.devices.some((d) => d.name === this.o.name);
      let h = "<h4>Omakey</h4><div class='tag'>Your phone is the keyboard.</div>" +
        "<div class='sec'>Omarchy</div><div class='note'>On Omarchy, click the keyboard icon in the bar and choose Pair a phone. Then scan the code.</div>" +
        "<div class='row'><button class='primary' data-a='scan'>Scan QR code</button><button data-a='paste'>Paste link</button></div>" +
        "<div class='sec'>Paired computers</div>";
      h += paired ? "<div class='card ok' data-a='open'><b>" + esc(S.hostName) + "</b><span>● connected · 4 ms · tap to type</span></div>" : "<div class='note' style='color:var(--p-dim)'>No computers yet.</div>";
      h += "<div class='sec'>Nearby</div>" + (paired ? "<div class='note' style='color:var(--p-dim)'>Looking for computers running omakeyd…</div>"
        : "<div class='card'><b>" + esc(S.hostName) + "</b><span>Not paired. Scan its pairing code to connect.</span></div>");
      if (this.toast) h += "<div style='position:absolute;left:8%;right:8%;bottom:4%;background:#000c;color:#fff;border-radius:20px;padding:.7em 1em;text-align:center'>" + esc(this.toast) + "</div>";
      this.cn.innerHTML = h;
    }
    say(t) { this.toast = t; this.render(); clearTimeout(this.tt); this.tt = setTimeout(() => { this.toast = null; this.render(); }, 2600); }
    onClick(e) {
      const b = e.target.closest("[data-a]");
      if (!b) return;
      const a = b.dataset.a;
      if (a === "scan") return this.scan();
      if (a === "paste") {
        if (!S.pairing || !S.clipboard) return this.say("The clipboard is empty. Copy the pairing link first.");
        return this.confirm();
      }
      if (a === "open") return this.say("Opening the keyboard…");
      if (a === "cancel-scan") { this.scanning = false; return this.render(); }
      if (a === "cancel") { this.dialog = false; return this.render(); }
      if (a === "pair") {
        this.dialog = false;
        if (OMK.completePairing(this.o.name)) { this.say("Paired with " + S.hostName); this.o.onAction("paired"); }
        else this.say("That pairing code expired. Pair again.");
      }
    }
    scan() {
      this.scanning = true;
      this.cn.innerHTML += "<div class='scan'><div class='vf'></div><p>Scan the Omakey pairing code</p><button data-a='cancel-scan' style='position:absolute;top:4%;right:5%;background:none;border:0;color:#fff;font-size:1.6em;cursor:pointer'>×</button></div>";
      this.o.onAction("scan");
      if (S.pairing) this.found();
      else this.cn.querySelector(".scan p").textContent = "Waiting for a code: click Pair a phone on the desktop";
    }
    found() {
      const vf = this.cn.querySelector(".vf");
      if (!vf) return;
      vf.innerHTML = '<div style="position:absolute;inset:12%;color:#fff">' + OMK.qr(S.pairing.seed, 200) + "</div>";
      this.cn.querySelector(".scan p").textContent = "Scan the Omakey pairing code";
      setTimeout(() => { if (this.scanning) { this.scanning = false; this.confirm(); } }, 1100);
    }
    confirm() {
      this.scanning = false;
      this.dialog = false;
      this.render();
      if (!S.pairing) return;
      this.dialog = true;
      const d = el("div", "ph-dialog", "<div><h4>Pair with " + esc(S.hostName) + "?</h4><p>Computer: " + esc(S.hostName) + "</p><p>Addresses: 192.168.1.20, fd7a:115c::7</p>" +
        "<div class='fp'>" + S.pairing.fp + "</div><p>It must match the code under the QR code on your computer.</p>" +
        "<div class='acts'><button data-a='cancel'>Cancel</button><button class='primary' data-a='pair'>Pair</button></div></div>");
      this.cn.appendChild(d);
    }
  }
  OMK.ConnectPhone = ConnectPhone;
})();
