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
  readonly property string expectedVersion: "0.3.0"
  readonly property string pluginDir: String(Qt.resolvedUrl(".")).replace(/^file:\/\//, "").replace(/\/$/, "")
  readonly property string statePath: (Quickshell.env("XDG_RUNTIME_DIR") || "/tmp") + "/omakey/state.json"

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

  // ---- panel UI state ----
  property int pairedAtStart: -1
  readonly property bool justPaired: pairedAtStart >= 0 && pairedCount > pairedAtStart
  property bool busy: false
  property string renamingId: ""
  property string confirmForgetId: ""
  property bool settingsOpen: false
  property bool copied: false

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
  function startPairing() {
    root.pairedAtStart = root.pairedCount
    run(["pair", "--no-wait"])
  }
  function cancelPairing() { run(["cancel-pair"]) }
  function copyLink() {
    if (!root.pairing) return
    Quickshell.execDetached(["wl-copy", root.pairing.uri])
    root.copied = true
    copiedTimer.restart()
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
        color: button.active && button.useActiveColor ? button.activeColor : button.foreground
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
                color: root.connected > 0 ? Color.accent
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
              visible: root.running
              // nf-md-power
              iconText: "󰐥"
              tooltipText: "Stop the service"
              foreground: root.bar.foreground
              onClicked: root.systemctl("stop")
            }
          }
        }

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
        Column {
          visible: root.pairing !== null
          width: parent.width
          spacing: Style.space(10)

          Rectangle {
            // QR codes need dark-on-light to scan reliably, whatever the theme.
            color: "#ffffff"
            radius: Style.space(6)
            width: Math.min(parent.width, Style.space(250))
            height: width
            anchors.horizontalCenter: parent.horizontalCenter

            Canvas {
              id: qr
              anchors.fill: parent
              property var rows: root.pairing ? root.pairing.qr : []
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

          Text {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: "One phone · expires in " + Math.floor(root.secondsLeft / 60) + ":" + ("0" + (root.secondsLeft % 60)).slice(-2)
            textFormat: Text.PlainText
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
                color: deviceRow.live ? Color.accent : Util.alpha(root.bar.foreground, 0.25)
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
                    : deviceRow.live ? "Connected · " + deviceRow.addr + (deviceRow.held > 0 ? " · " + deviceRow.held + " held" : "")
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

        // ---- settings ----
        PanelSeparator {
          visible: root.installed && root.probed && root.phase !== "missing"
          foreground: root.bar.foreground
        }

        Column {
          visible: root.installed && root.probed && root.phase !== "missing"
          width: parent.width
          spacing: Style.space(10)

          Item {
            width: parent.width
            height: settingsHeader.implicitHeight

            PanelSectionHeader {
              id: settingsHeader
              anchors.left: parent.left
              anchors.verticalCenter: parent.verticalCenter
              text: "SETTINGS"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
            }

            Text {
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              // nf-md-chevron_down / chevron_up
              text: root.settingsOpen ? "󰅃" : "󰅀"
              color: Util.alpha(root.bar.foreground, 0.6)
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.body
            }

            MouseArea {
              anchors.fill: parent
              cursorShape: Qt.PointingHandCursor
              onClicked: {
                root.settingsOpen = !root.settingsOpen
                if (root.settingsOpen) {
                  hostField.text = root.state.host_name || ""
                  portField.text = String(root.state.port || 47800)
                }
              }
            }
          }

          Column {
            visible: root.settingsOpen
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
        }

        // ---- footer ----
        Text {
          visible: root.running
          width: parent.width
          text: (root.state.host_name || "") + " · UDP " + (root.state.port || "")
                + (root.state.bluetooth ? " · Bluetooth" : "") + "\n" + (root.state.addresses || []).join(", ")
          textFormat: Text.PlainText
          wrapMode: Text.WordWrap
          lineHeight: 1.2
          color: Util.alpha(root.bar.foreground, 0.45)
          font.family: root.bar.fontFamily
          font.pixelSize: Style.font.caption
        }
      }
    }
  }
}
