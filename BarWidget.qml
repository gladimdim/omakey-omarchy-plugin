import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

// Bar button and control panel for Omakey: set up the service, pair phones
// with a QR code, rename or forget them, and change settings.
//
// omakeyd (a systemd user service) does the work. This reads the state file
// it publishes and drives it through its CLI, so it works on any bar and the
// keyboard keeps working across shell restarts.
Panel {
  id: root
  moduleName: "gladimdim.omakey"
  ipcTarget: "gladimdim.omakey"

  readonly property string bin: "/usr/local/bin/omakeyd"
  // The omakeyd version this widget expects; an older one gets an Update button.
  readonly property string expectedVersion: "1.4.0"
  readonly property string pluginDir: String(Qt.resolvedUrl(".")).replace(/^file:\/\//, "").replace(/\/$/, "")
  // Like omakeyd, never fall back to a shared directory such as /tmp.
  readonly property string statePath: Quickshell.env("XDG_RUNTIME_DIR")
    ? Quickshell.env("XDG_RUNTIME_DIR") + "/omakey/state.json" : ""

  // ---- what the daemon publishes ----
  property var state: ({})
  property double now: Date.now() / 1000

  // ---- what the probe finds ----
  property bool probed: false
  property bool installed: true
  property string service: ""      // systemd ActiveState: active, inactive, failed, activating…
  property bool pidAlive: false

  readonly property bool running: state.running === true && pidAlive
  readonly property bool failed: service === "failed"
  readonly property bool starting: !running && (service === "activating" || service === "reloading" || busy)
  readonly property int connected: running ? (state.connected || 0) : 0
  readonly property var pairing: running && state.pairing ? state.pairing : null
  readonly property int secondsLeft: pairing ? Math.max(0, Math.round(pairing.expires_at - now)) : 0
  readonly property int pairedCount: state.paired ? state.paired.count : 0
  readonly property bool needsUpdate: running && state.daemon_version !== expectedVersion
  // {state: "on" | "off" | "unavailable", address, reason}
  readonly property var bluetooth: state.bluetooth && typeof state.bluetooth === "object" ? state.bluetooth : null
  readonly property string bluetoothText: !bluetooth ? ""
    : bluetooth.state === "on" ? "Bluetooth on"
    : "Bluetooth off" + (bluetooth.reason ? " (" + bluetooth.reason + ")" : "")
  // {interface, kind: "wifi" | "ethernet", mac, supported, on, reason}
  readonly property var wake: state.wake_on_lan && typeof state.wake_on_lan === "object" ? state.wake_on_lan : null
  readonly property string wakeText: !wake ? ""
    : wake.on ? "Phones on this network can wake it from sleep" + (wake.kind === "ethernet" ? " or power-off" : "")
                + " (" + wake.interface + ")"
    : wake.supported ? "Off. Turning it on asks for your password."
    : "Not available: " + wake.reason

  // Connected: green whatever the theme (some themes' "green" is amber).
  readonly property color connectedColor: "#4caf50"

  // ---- panel UI state ----
  property int pairedAtStart: -1
  readonly property bool justPaired: pairedAtStart >= 0 && pairedCount > pairedAtStart
  property bool busy: false
  property string renamingId: ""
  property string confirmForgetId: ""
  // The settings page shows instead of the main one.
  property bool settingsOpen: false
  // This plugin's version, from its manifest.
  property string pluginVersion: ""
  property bool copied: false
  // The phone app whose link shows: "", "android" or "ios".
  property string appShown: ""
  readonly property bool appOpen: appShown !== ""
  readonly property var app: appShown === "ios" ? iosApp : androidApp
  property bool appCopied: false

  // The phone apps: where to get each, and that link as a QR code (made
  // with qrencode, one string of 0/1 per row). Built in, so they show even
  // before the service is set up, when the phone needs them most.
  readonly property var androidApp: ({
    url: "https://github.com/gladimdim/omakey-mobile/releases/latest",
    about: "Scan with your phone's camera to download the latest Omakey APK, then open it to install.",
    qr: [
      "111111100100000110011001001111111",
      "100000101101100001101110001000001",
      "101110100111001010111110101011101",
      "101110101101000001010001101011101",
      "101110100010111100110001001011101",
      "100000101010011100001010101000001",
      "111111101010101010101010101111111",
      "000000000000110110001110000000000",
      "111110111110101111000010110101010",
      "000100000100000011111001001000111",
      "010010110101100110000010000111010",
      "000110000111001000101110011010100",
      "011000111101000111010011110111000",
      "010100001110111010111001001100011",
      "100001110010011110001110011110010",
      "011100010010110110000110111100100",
      "010001111110101101010011110110010",
      "011111001100000010111101001001011",
      "101001110111100100100100111001010",
      "010110000011001110010110010000100",
      "111111110111000111000010100010010",
      "101111011000111010011001001001011",
      "100010110110011111101000011101010",
      "100001001010110100011110111111100",
      "100000100000101101010010111110001",
      "000000001110000010111100100011101",
      "111111101001100100001111101011010",
      "100000100001001110001111100011101",
      "101110101101000001010010111110001",
      "101110101010111010011010110110011",
      "101110101110011111101101001001100",
      "100000101110110010001110010101100",
      "111111101110101001110001111001010"
    ]
  })
  // Open source, built in Xcode: the link is its source and build steps.
  readonly property var iosApp: ({
    url: "https://github.com/gladimdim/omakey-mobile-ios",
    about: "Scan with your iPhone's camera for the source and build steps: build it in Xcode (iOS 17 or newer).",
    qr: [
      "11111110100110100110001111111",
      "10000010011001001001001000001",
      "10111010001011110011001011101",
      "10111010000011011100101011101",
      "10111010010101100011001011101",
      "10000010001011100110001000001",
      "11111110101010101010101111111",
      "00000000101101000001000000000",
      "11011010011100100100001000001",
      "01100100110010110111010110110",
      "10010011101101111100000110100",
      "11111100111110000101111101001",
      "11001010110110011001101100001",
      "01000100000000111010001111111",
      "00101011111111010001111010101",
      "10011101001100011011000110101",
      "01111010001011000001110001000",
      "11010000100111010011000010110",
      "11011011101101010111000011001",
      "11100001001111100001001001100",
      "11011111101000111100111111110",
      "00000000110100110110100011000",
      "11111110011100011101101011000",
      "10000010000000100101100010000",
      "10111010111010101011111111000",
      "10111010100100111110010000001",
      "10111010010001110100110110111",
      "10000010100010010000001101101",
      "11111110100011000001101010000"
    ]
  })

  // A QR code: dark on light, which phones scan reliably whatever the theme.
  component QrCode: Rectangle {
    property var rows: []
    color: "#ffffff"
    radius: Style.space(6)

    Canvas {
      anchors.fill: parent
      property var rows: parent.rows
      onRowsChanged: requestPaint()
      onWidthChanged: requestPaint()
      onPaint: {
        var ctx = getContext("2d")
        ctx.reset()
        var n = rows.length
        if (!n) return
        // Whole-pixel modules keep the edges crisp; 3 modules of margin.
        var cell = Math.floor(width / (n + 6))
        var off = Math.floor((width - cell * n) / 2)
        ctx.fillStyle = "#000000"
        for (var y = 0; y < n; y++) {
          var row = rows[y]
          for (var x = 0; x < n; x++) {
            if (row.charAt(x) === "1") ctx.fillRect(off + x * cell, off + y * cell, cell, cell)
          }
        }
      }
    }
  }

  readonly property string phase: {
    if (!probed) return "probing"
    if (!installed) return "missing"
    if (running) return "running"
    if (starting) return "starting"
    if (failed) return "failed"
    return "stopped"
  }

  readonly property string statusText: {
    switch (phase) {
    case "probing": return "Checking…"
    case "missing": return "Set up Omakey once to use your phone as a keyboard. It asks for your password in a terminal."
    case "starting": return "Starting…"
    case "failed": return "The service stopped with an error. If you just installed it, log out and back in once, then start it again."
    case "stopped": return "The service is off. Start it to type from your phone."
    }
    if (pairing) return "Open Omakey on your phone, tap Scan QR code, and point it here."
    if (justPaired) return "Paired with " + (state.paired.name || "your phone") + ". Start typing."
    if (connected === 1) return "1 phone is connected."
    if (connected > 1) return connected + " phones are connected."
    if (devicesModel.count === 0) return "Pair your phone to use it as a keyboard."
    return "Ready. Open Omakey on your phone to connect."
  }

  // ---- reading state ----
  FileView {
    path: root.pluginDir + "/manifest.json"
    watchChanges: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: {
      try { root.pluginVersion = JSON.parse(text()).version || "" } catch (e) {}
    }
  }

  FileView {
    id: stateFile
    path: root.statePath
    watchChanges: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: {
      // A read that races a write can see half a file; keep what we had.
      try { root.applyState(JSON.parse(text()) || {}) } catch (e) {}
    }
    onLoadFailed: root.applyState({})
  }

  // Devices live in a ListModel updated in place, so a rename field or a
  // forget prompt survives the state file being rewritten.
  ListModel { id: devicesModel }

  function applyState(s) {
    root.state = s
    var list = s.devices || []
    var ids = {}
    for (var i = 0; i < list.length; i++) {
      var d = list[i]
      ids[d.id] = true
      var row = {
        deviceId: d.id,
        name: d.name || "Phone",
        online: d.connected === true,
        addr: d.addr || "",
        transport: d.transport || "",
        loss: typeof d.loss === "number" ? d.loss : -1,
        held: d.held || 0,
        lastSeen: d.last_seen || 0,
      }
      var at = -1
      for (var j = 0; j < devicesModel.count; j++) {
        if (devicesModel.get(j).deviceId === d.id) { at = j; break }
      }
      if (at < 0) {
        devicesModel.insert(Math.min(i, devicesModel.count), row)
      } else {
        if (at !== i && i < devicesModel.count) devicesModel.move(at, i, 1)
        for (var k in row) {
          if (devicesModel.get(i)[k] !== row[k]) devicesModel.setProperty(i, k, row[k])
        }
      }
    }
    for (var m = devicesModel.count - 1; m >= 0; m--) {
      if (!ids[devicesModel.get(m).deviceId]) devicesModel.remove(m)
    }
    if (root.renamingId && !ids[root.renamingId]) root.renamingId = ""
    if (root.confirmForgetId && !ids[root.confirmForgetId]) root.confirmForgetId = ""
  }

  // Is the binary installed, is the service up, is the daemon alive?
  Process {
    id: probe
    command: ["sh", "-c",
      "[ -x \"$1\" ] && echo bin=1 || echo bin=0; " +
      "echo service=$(systemctl --user is-active omakeyd 2>/dev/null); " +
      "[ \"$2\" -gt 0 ] 2>/dev/null && kill -0 \"$2\" 2>/dev/null && echo pid=1 || echo pid=0",
      "sh", root.bin, String(root.state.pid || 0)]
    stdout: StdioCollector { id: probeOut }
    onExited: {
      var lines = probeOut.text.split("\n")
      for (var i = 0; i < lines.length; i++) {
        var kv = lines[i].split("=")
        if (kv[0] === "bin") root.installed = kv[1] === "1"
        else if (kv[0] === "service") root.service = (kv[1] || "").trim()
        else if (kv[0] === "pid") root.pidAlive = kv[1] === "1"
      }
      root.probed = true
      if (root.running || root.failed || !root.installed) root.busy = false
    }
  }

  function refresh() {
    stateFile.reload()
    if (!probe.running) probe.running = true
  }

  Timer {
    interval: root.opened || root.busy ? 1000 : 10000
    repeat: true
    running: true
    triggeredOnStart: true
    onTriggered: {
      root.now = Date.now() / 1000
      root.refresh()
    }
  }

  // A pairing countdown needs a ticking clock even between probes.
  Timer {
    interval: 1000
    repeat: true
    running: root.pairing !== null
    onTriggered: root.now = Date.now() / 1000
  }

  Timer {
    id: busyTimeout
    interval: 15000
    onTriggered: root.busy = false
  }

  Timer {
    id: copiedTimer
    interval: 1500
    onTriggered: root.copied = false
  }

  Timer {
    id: appCopiedTimer
    interval: 1500
    onTriggered: root.appCopied = false
  }

  // ---- actions ----
  function run(args) { Quickshell.execDetached([root.bin].concat(args)) }
  function systemctl(verb) {
    root.busy = verb !== "stop"
    busyTimeout.restart()
    Quickshell.execDetached(["systemctl", "--user", verb, "omakeyd"])
    Qt.callLater(root.refresh)
  }
  function inTerminal(script) {
    Quickshell.execDetached(["omarchy-launch-floating-terminal-with-presentation", script])
  }
  function install() {
    root.busy = true
    busyTimeout.restart()
    inTerminal(Util.shellQuote(root.pluginDir + "/install.sh"))
    root.close()
  }
  function showLogs() { inTerminal("journalctl --user -u omakeyd -n 100 -f") }
  // Needs root: the CLI asks for the sudo password in a terminal.
  function setWake(on) {
    inTerminal(Util.shellQuote(root.bin) + " wake-on-lan " + (on ? "on" : "off"))
    root.close()
  }
  function startPairing() {
    root.pairedAtStart = root.pairedCount
    run(["pair", "--no-wait"])
  }
  function cancelPairing() { run(["cancel-pair"]) }
  // The link is a phone's key: copy it marked sensitive (clipboard managers
  // skip it), pass it through the environment rather than argv (which other
  // users can read in ps), and clear it after a minute if it's still there.
  function copyLink() {
    if (!root.pairing) return
    Quickshell.execDetached({
      command: ["sh", "-c",
        "printf %s \"$OMAKEY_LINK\" | wl-copy --sensitive || exit; sleep 60; " +
        "[ \"$(wl-paste --no-newline 2>/dev/null)\" = \"$OMAKEY_LINK\" ] && wl-copy --clear"],
      environment: { OMAKEY_LINK: root.pairing.uri },
    })
    root.copied = true
    copiedTimer.restart()
  }
  function openAppPage() {
    Quickshell.execDetached(["xdg-open", root.app.url])
    root.close()
  }
  function copyAppLink() {
    Quickshell.execDetached(["wl-copy", root.app.url])
    root.appCopied = true
    appCopiedTimer.restart()
  }
  function forget(id) {
    root.confirmForgetId = ""
    run(["forget", id])
  }
  function rename(id, name) {
    root.renamingId = ""
    var n = String(name).trim()
    if (n.length > 0) run(["rename", id, n])
  }
  function openSettings() {
    hostField.text = root.state.host_name || ""
    portField.text = String(root.state.port || 47800)
    root.settingsOpen = true
  }
  function saveSettings(name, port) {
    root.busy = true
    busyTimeout.restart()
    // Values are passed as positional arguments, never spliced into the script.
    Quickshell.execDetached(["sh", "-c",
      "\"$1\" config --name \"$2\" --port \"$3\" && systemctl --user restart omakeyd",
      "sh", root.bin, String(name).trim(), String(port).trim()])
    root.settingsOpen = false
  }

  function ago(ts) {
    if (!ts) return "never"
    var s = Math.max(0, root.now - ts)
    if (s < 90) return "just now"
    if (s < 3600) return Math.round(s / 60) + " min ago"
    if (s < 86400 * 2) return Math.round(s / 3600) + " h ago"
    return Math.round(s / 86400) + " days ago"
  }

  onOpenedChanged: {
    if (opened) {
      root.refresh()
    } else {
      root.pairedAtStart = -1
      root.renamingId = ""
      root.confirmForgetId = ""
      root.settingsOpen = false
      root.appShown = ""
    }
  }

  // ---- bar button ----
  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    iconComponent: Component {
      OmakeyIcon {
        color: button.foreground
        lit: root.connected > 0
      }
    }
    active: root.connected > 0
    dimmed: root.phase !== "running" && root.phase !== "probing"
    tooltipText: root.opened ? "" : (root.connected > 0
      ? "Omakey · " + root.connected + (root.connected === 1 ? " phone" : " phones") + " connected"
      : "Omakey · right-click to pair a phone")
    onPressed: function(b) {
      if (b === Qt.RightButton && root.running) {
        root.startPairing()
        root.open()
      } else {
        root.toggle()
      }
    }
  }

  // ---- panel ----
  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(340))
    contentHeight: panel.fittedContentHeight(column.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: {
        if (root.renamingId || root.confirmForgetId) {
          root.renamingId = ""
          root.confirmForgetId = ""
        } else if (root.settingsOpen) {
          root.settingsOpen = false
        } else {
          root.close()
        }
      }
      onTabRequested: function(direction) { root.switchPanel(direction) }

      Column {
        id: column
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        spacing: Style.space(14)

        // ---- header ----
        Item {
          visible: !root.settingsOpen
          width: parent.width
          height: Math.max(titleBlock.implicitHeight, headerActions.height)

          Column {
            id: titleBlock
            anchors.left: parent.left
            anchors.right: headerActions.left
            anchors.rightMargin: Style.space(8)
            spacing: Style.space(4)

            Row {
              spacing: Style.space(8)

              OmakeyIcon {
                width: Style.space(24)
                height: width
                anchors.verticalCenter: parent.verticalCenter
                color: Color.accent
              }

              Text {
                text: "Omakey"
                textFormat: Text.PlainText
                color: root.bar.foreground
                font.family: root.bar.fontFamily
                font.pixelSize: Style.font.title
                font.bold: true
              }

              Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: Style.space(7)
                height: width
                radius: width / 2
                color: root.connected > 0 ? root.connectedColor
                  : root.phase === "running" ? Util.alpha(root.bar.foreground, 0.5)
                  : root.phase === "failed" ? Color.urgent
                  : Util.alpha(root.bar.foreground, 0.2)
              }
            }

            Text {
              width: parent.width
              text: root.statusText
              textFormat: Text.PlainText
              color: Util.alpha(root.bar.foreground, 0.7)
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
              wrapMode: Text.WordWrap
            }
          }

          Row {
            id: headerActions
            anchors.right: parent.right
            anchors.top: parent.top
            spacing: Style.space(2)
            visible: root.installed && root.probed

            PanelActionButton {
              // nf-md-text_box_outline
              iconText: "󰦨"
              tooltipText: "Show logs"
              foreground: root.bar.foreground
              onClicked: root.showLogs()
            }

            PanelActionButton {
              visible: root.running
              // nf-md-restart
              iconText: "󰜉"
              tooltipText: "Restart the service"
              foreground: root.bar.foreground
              onClicked: root.systemctl("restart")
            }

            PanelActionButton {
              visible: root.phase !== "missing"
              // nf-md-cog
              iconText: "󰒓"
              tooltipText: "Settings"
              foreground: root.bar.foreground
              onClicked: root.openSettings()
            }

            PanelActionButton {
              visible: root.running
              // nf-md-power
              iconText: "󰐥"
              tooltipText: "Stop the service"
              foreground: root.bar.foreground
              onClicked: root.systemctl("stop")
            }
          }
        }

        // ---- main page ----
        Column {
          visible: !root.settingsOpen
          width: parent.width
          spacing: Style.space(14)

          // ---- setup / start / update ----
          Button {
            visible: root.needsUpdate
            width: parent.width
            text: root.busy ? "Updating in the terminal…" : "Update the service"
            tooltipText: "This widget needs omakeyd " + root.expectedVersion + "; it asks for your password in a terminal"
            iconText: "󰚰"
            foreground: Color.accent
            fontFamily: root.bar.fontFamily
            bordered: true
            enabled: !root.busy
            onClicked: root.install()
          }

          Button {
            visible: root.phase === "missing"
            width: parent.width
            text: root.busy ? "Installing in the terminal…" : "Set up Omakey"
            iconText: "󰏗"
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
            bordered: true
            enabled: !root.busy
            onClicked: root.install()
          }

          Button {
            visible: root.phase === "stopped" || root.phase === "failed"
            width: parent.width
            text: root.phase === "failed" ? "Try again" : "Start Omakey"
            iconText: "󰐊"
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
            bordered: true
            onClicked: root.systemctl(root.phase === "failed" ? "restart" : "start")
          }

          // ---- pairing ----
          // The app's QR code takes this one's place while it's open: get the
          // app first, then pair.
          Column {
            visible: root.pairing !== null && !root.appOpen
            width: parent.width
            spacing: Style.space(10)

            QrCode {
              rows: root.pairing ? root.pairing.qr : []
              width: Math.min(parent.width, Style.space(250))
              height: width
              anchors.horizontalCenter: parent.horizontalCenter
            }

            Text {
              width: parent.width
              horizontalAlignment: Text.AlignHCenter
              text: "One phone · expires in " + Math.floor(root.secondsLeft / 60) + ":" + ("0" + (root.secondsLeft % 60)).slice(-2)
              textFormat: Text.PlainText
              color: Util.alpha(root.bar.foreground, 0.6)
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
            }

            Text {
              visible: !!(root.pairing && root.pairing.fingerprint)
              width: parent.width
              horizontalAlignment: Text.AlignHCenter
              text: "Fingerprint " + (root.pairing ? root.pairing.fingerprint : "") + " · the phone shows the same code"
              textFormat: Text.PlainText
              wrapMode: Text.WordWrap
              color: Util.alpha(root.bar.foreground, 0.6)
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
            }

            Row {
              id: pairActions
              width: parent.width
              spacing: Style.space(6)
              readonly property real cellWidth: (width - spacing) / 2

              Button {
                width: pairActions.cellWidth
                text: root.copied ? "Copied" : "Copy link"
                iconText: "󰆏"
                tooltipText: "For pasting into the app when the camera can't scan"
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
                bordered: true
                onClicked: root.copyLink()
              }

              Button {
                width: pairActions.cellWidth
                text: "Cancel"
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
                bordered: true
                onClicked: root.cancelPairing()
              }
            }
          }

          Button {
            visible: root.running && root.pairing === null
            width: parent.width
            text: devicesModel.count === 0 ? "Pair a phone" : "Pair another phone"
            // nf-md-qrcode
            iconText: "󰐲"
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
            bordered: true
            onClicked: root.startPairing()
          }

          // ---- the phone apps ----
          Row {
            id: appButtons
            width: parent.width
            spacing: Style.space(6)
            readonly property real cellWidth: (width - spacing) / 2
            readonly property string closeText: root.pairing ? "Back to pairing" : "Hide link"

            Button {
              width: appButtons.cellWidth
              text: root.appShown === "android" ? appButtons.closeText : "Android app"
              // nf-md-android
              iconText: "󰀲"
              tooltipText: "A QR code and link to the latest Omakey APK"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              onClicked: root.appShown = root.appShown === "android" ? "" : "android"
            }

            Button {
              width: appButtons.cellWidth
              text: root.appShown === "ios" ? appButtons.closeText : "iPhone app"
              // nf-md-apple
              iconText: "󰀵"
              tooltipText: "A QR code and link to the iPhone app"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              onClicked: root.appShown = root.appShown === "ios" ? "" : "ios"
            }
          }

          Column {
            visible: root.appOpen
            width: parent.width
            spacing: Style.space(10)

            QrCode {
              rows: root.app.qr
              width: Math.min(parent.width, Style.space(220))
              height: width
              anchors.horizontalCenter: parent.horizontalCenter
            }

            Text {
              width: parent.width
              horizontalAlignment: Text.AlignHCenter
              text: root.app.about
              textFormat: Text.PlainText
              wrapMode: Text.WordWrap
              color: Util.alpha(root.bar.foreground, 0.6)
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
            }

            Row {
              id: appActions
              width: parent.width
              spacing: Style.space(6)
              readonly property real cellWidth: (width - spacing) / 2

              Button {
                width: appActions.cellWidth
                text: "Open in browser"
                // nf-md-open_in_new
                iconText: "󰏌"
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
                bordered: true
                onClicked: root.openAppPage()
              }

              Button {
                width: appActions.cellWidth
                text: root.appCopied ? "Copied" : "Copy link"
                iconText: "󰆏"
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
                bordered: true
                onClicked: root.copyAppLink()
              }
            }
          }

          // ---- phones ----
          PanelSeparator {
            visible: devicesModel.count > 0
            foreground: root.bar.foreground
          }

          Column {
            visible: devicesModel.count > 0
            width: parent.width
            spacing: Style.space(10)

            PanelSectionHeader {
              text: "PHONES"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
            }

            Repeater {
              model: devicesModel

              Item {
                id: deviceRow
                required property string deviceId
                required property string name
                required property bool online
                required property string addr
                required property string transport
                required property real loss
                required property int held
                required property real lastSeen
                readonly property bool live: online && root.running
                readonly property bool renaming: root.renamingId === deviceId
                readonly property bool confirming: root.confirmForgetId === deviceId

                width: parent.width
                height: Math.max(info.implicitHeight, actions.height)

                Rectangle {
                  id: dot
                  width: Style.space(7)
                  height: width
                  radius: width / 2
                  anchors.left: parent.left
                  anchors.verticalCenter: parent.verticalCenter
                  color: deviceRow.live ? root.connectedColor : Util.alpha(root.bar.foreground, 0.25)
                }

                Column {
                  id: info
                  anchors.left: dot.right
                  anchors.leftMargin: Style.space(10)
                  anchors.right: actions.left
                  anchors.rightMargin: Style.space(8)
                  anchors.verticalCenter: parent.verticalCenter
                  spacing: Style.space(2)

                  Text {
                    visible: !deviceRow.renaming
                    width: parent.width
                    text: deviceRow.name
                    textFormat: Text.PlainText
                    elide: Text.ElideRight
                    color: root.bar.foreground
                    font.family: root.bar.fontFamily
                    font.pixelSize: Style.font.body
                  }

                  TextField {
                    id: nameField
                    visible: deviceRow.renaming
                    width: parent.width
                    foreground: root.bar.foreground
                    placeholderText: "Phone name"
                    maximumLength: 64
                    onVisibleChanged: if (visible) {
                      text = deviceRow.name
                      forceActiveFocus()
                      selectAll()
                    }
                    onAccepted: root.rename(deviceRow.deviceId, text)
                    Keys.onEscapePressed: root.renamingId = ""
                  }

                  Text {
                    width: parent.width
                    text: deviceRow.confirming ? "Forget this phone? It will need a new QR code."
                      : deviceRow.renaming ? "Enter to save, Esc to cancel"
                      : deviceRow.live ? "Connected · "
                        + (deviceRow.transport === "bluetooth" ? "Bluetooth" : "Wi-Fi " + deviceRow.addr)
                        + (deviceRow.loss >= 1 ? " · " + Math.round(deviceRow.loss) + "% lost" : "")
                        + (deviceRow.held > 0 ? " · " + deviceRow.held + " held" : "")
                      : "Last seen " + root.ago(deviceRow.lastSeen)
                    textFormat: Text.PlainText
                    elide: Text.ElideRight
                    wrapMode: deviceRow.confirming ? Text.WordWrap : Text.NoWrap
                    color: deviceRow.confirming ? Color.urgent : Util.alpha(root.bar.foreground, 0.6)
                    font.family: root.bar.fontFamily
                    font.pixelSize: Style.font.caption
                  }
                }

                Row {
                  id: actions
                  anchors.right: parent.right
                  anchors.verticalCenter: parent.verticalCenter
                  spacing: Style.space(2)

                  // Normal: rename, forget.
                  PanelActionButton {
                    visible: !deviceRow.renaming && !deviceRow.confirming
                    // nf-md-pencil
                    iconText: "󰏫"
                    tooltipText: "Rename"
                    foreground: root.bar.foreground
                    enabled: root.running
                    onClicked: {
                      root.confirmForgetId = ""
                      root.renamingId = deviceRow.deviceId
                    }
                  }

                  PanelActionButton {
                    visible: !deviceRow.renaming && !deviceRow.confirming
                    // nf-md-link_off
                    iconText: "󰌸"
                    tooltipText: "Forget this phone"
                    foreground: root.bar.foreground
                    enabled: root.running
                    onClicked: {
                      root.renamingId = ""
                      root.confirmForgetId = deviceRow.deviceId
                    }
                  }

                  // Renaming: save.
                  PanelActionButton {
                    visible: deviceRow.renaming
                    // nf-md-check
                    iconText: "󰄬"
                    tooltipText: "Save"
                    foreground: root.bar.foreground
                    onClicked: root.rename(deviceRow.deviceId, nameField.text)
                  }

                  // Confirming: forget / keep.
                  Button {
                    visible: deviceRow.confirming
                    text: "Forget"
                    foreground: Color.urgent
                    fontFamily: root.bar.fontFamily
                    fontSize: Style.font.bodySmall
                    bordered: true
                    onClicked: root.forget(deviceRow.deviceId)
                  }

                  Button {
                    visible: deviceRow.confirming
                    text: "Keep"
                    foreground: root.bar.foreground
                    fontFamily: root.bar.fontFamily
                    fontSize: Style.font.bodySmall
                    onClicked: root.confirmForgetId = ""
                  }
                }
              }
            }
          }
        }

        // ---- settings page ----
        Column {
          visible: root.settingsOpen
          width: parent.width
          spacing: Style.space(14)

          Row {
            spacing: Style.space(8)

            PanelActionButton {
              anchors.verticalCenter: parent.verticalCenter
              // nf-md-arrow_left
              iconText: "󰁍"
              tooltipText: "Back"
              foreground: root.bar.foreground
              onClicked: root.settingsOpen = false
            }

            Text {
              anchors.verticalCenter: parent.verticalCenter
              text: "Settings"
              textFormat: Text.PlainText
              color: root.bar.foreground
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.title
              font.bold: true
            }
          }

          Item {
            visible: root.wake !== null
            width: parent.width
            height: Math.max(wakeTexts.implicitHeight, wakeButton.implicitHeight)

            Column {
              id: wakeTexts
              anchors.left: parent.left
              anchors.right: wakeButton.visible ? wakeButton.left : parent.right
              anchors.rightMargin: wakeButton.visible ? Style.space(8) : 0
              anchors.verticalCenter: parent.verticalCenter
              spacing: Style.space(2)

              Text {
                text: "Wake on LAN"
                textFormat: Text.PlainText
                color: root.bar.foreground
                font.family: root.bar.fontFamily
                font.pixelSize: Style.font.body
              }

              Text {
                width: parent.width
                text: root.wakeText
                textFormat: Text.PlainText
                wrapMode: Text.WordWrap
                color: Util.alpha(root.bar.foreground, 0.7)
                font.family: root.bar.fontFamily
                font.pixelSize: Style.font.caption
              }
            }

            Button {
              id: wakeButton
              visible: root.wake !== null && (root.wake.supported || root.wake.on)
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              text: root.wake && root.wake.on ? "Turn off" : "Turn on"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              fontSize: Style.font.bodySmall
              bordered: true
              onClicked: root.setWake(!root.wake.on)
            }
          }

          PanelSeparator {
            visible: root.wake !== null
            foreground: root.bar.foreground
          }

          Column {
            width: parent.width
            spacing: Style.space(8)

            Text {
              text: "Name your phone shows"
              textFormat: Text.PlainText
              color: Util.alpha(root.bar.foreground, 0.7)
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
            }

            TextField {
              id: hostField
              width: parent.width
              foreground: root.bar.foreground
              placeholderText: "This computer"
              maximumLength: 64
            }

            Text {
              text: "UDP port (phones need a new QR code after a change)"
              textFormat: Text.PlainText
              color: Util.alpha(root.bar.foreground, 0.7)
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
            }

            TextField {
              id: portField
              width: parent.width
              foreground: root.bar.foreground
              placeholderText: "47800"
              maximumLength: 5
              validator: IntValidator { bottom: 1024; top: 65535 }
            }

            Button {
              width: parent.width
              text: "Save and restart"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              enabled: portField.acceptableInput
              onClicked: root.saveSettings(hostField.text, portField.text)
            }
          }

          PanelSeparator {
            foreground: root.bar.foreground
          }

          Column {
            width: parent.width
            spacing: Style.space(4)

            PanelSectionHeader {
              text: "THIS COMPUTER"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
            }

            Text {
              width: parent.width
              text: (root.running
                      ? (root.state.host_name || "") + " · UDP " + (root.state.port || "")
                        + (root.bluetoothText ? " · " + root.bluetoothText : "")
                        + "\n" + (root.state.addresses || []).join(", ") + "\n"
                      : "")
                    + "Plugin " + (root.pluginVersion || "?") + " · omakeyd "
                    + (root.running && root.state.daemon_version ? root.state.daemon_version : "not running")
              textFormat: Text.PlainText
              wrapMode: Text.WordWrap
              lineHeight: 1.2
              color: Util.alpha(root.bar.foreground, 0.7)
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
            }
          }
        }
      }
    }
  }
}
