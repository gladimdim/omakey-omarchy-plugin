import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

// Bar button and panel for Omakey. omakeyd (a systemd user service) does
// the work; this reads the state file it publishes and drives it through
// its CLI, so it works on any bar and survives shell restarts.
Panel {
  id: root
  moduleName: "gladimdim.omakey"
  ipcTarget: "gladimdim.omakey"

  readonly property string bin: "/usr/local/bin/omakeyd"
  readonly property string statePath: (Quickshell.env("XDG_RUNTIME_DIR") || "/tmp") + "/omakey/state.json"

  property var state: ({})
  property double now: Date.now() / 1000

  // The daemon rewrites the file every second while it runs, so an old
  // timestamp means it stopped or crashed.
  readonly property bool running: state.running === true && now - (state.updated_at || 0) < 5
  readonly property var devices: state.devices || []
  readonly property int connected: running ? (state.connected || 0) : 0
  readonly property var pairing: running && state.pairing ? state.pairing : null
  readonly property int secondsLeft: pairing ? Math.max(0, Math.round(pairing.expires_at - now)) : 0
  readonly property bool justPaired: pairedCount > pairedAtOpen && pairedAtOpen >= 0
  property int pairedAtOpen: -1
  readonly property int pairedCount: state.paired ? state.paired.count : 0

  readonly property string statusText: {
    if (!running) return "The Omakey service isn't running."
    if (state.uinput && state.uinput !== "ok") return "Keyboard: " + state.uinput
    if (pairing) return "Scan this with the Omakey app."
    if (connected === 1) return "1 phone is connected."
    if (connected > 1) return connected + " phones are connected."
    if (devices.length === 0) return "Pair your phone to use it as a keyboard."
    return "Open Omakey on your phone to connect."
  }

  FileView {
    id: stateFile
    path: root.statePath
    watchChanges: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: {
      // A read that races a write can see half a file; keep what we had.
      try { root.state = JSON.parse(text()) || {} } catch (e) {}
    }
    onLoadFailed: root.state = ({})
  }

  Timer {
    interval: 1000
    repeat: true
    running: true
    onTriggered: {
      root.now = Date.now() / 1000
      // A watch can't be set on a file that doesn't exist yet.
      if (!root.running) stateFile.reload()
    }
  }

  function run(args) { Quickshell.execDetached([root.bin].concat(args)) }
  function startPairing() {
    root.pairedAtOpen = root.pairedCount
    run(["pair", "--no-wait"])
  }
  function cancelPairing() { run(["cancel-pair"]) }
  function forget(id) { run(["forget", id]) }
  function startService() { Quickshell.execDetached(["systemctl", "--user", "start", "omakeyd"]) }

  function ago(ts) {
    if (!ts) return "never"
    var s = Math.max(0, root.now - ts)
    if (s < 90) return "just now"
    if (s < 3600) return Math.round(s / 60) + " min ago"
    if (s < 86400 * 2) return Math.round(s / 3600) + " h ago"
    return Math.round(s / 86400) + " days ago"
  }

  onOpenedChanged: if (!opened) root.pairedAtOpen = -1

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    // nf-md-keyboard / nf-md-keyboard_off
    text: root.running ? "󰌌" : "󰌐"
    active: root.connected > 0
    dimmed: !root.running
    tooltipText: root.opened ? "" : (root.connected > 0 ? "Omakey: phone connected" : "Omakey")
    onPressed: function(b) {
      if (b === Qt.RightButton) {
        root.startPairing()
        root.open()
      } else {
        root.toggle()
      }
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(320))
    contentHeight: panel.fittedContentHeight(column.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }

      Column {
        id: column
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        spacing: Style.space(14)

        Column {
          width: parent.width
          spacing: Style.space(4)

          Text {
            text: "Omakey"
            textFormat: Text.PlainText
            color: root.bar.foreground
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.title
            font.bold: true
          }

          Text {
            width: parent.width
            text: root.justPaired && !root.pairing ? "Paired with " + state.paired.name + ". Start typing." : root.statusText
            textFormat: Text.PlainText
            color: Util.alpha(root.bar.foreground, 0.7)
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.caption
            wrapMode: Text.WordWrap
          }
        }

        Button {
          visible: !root.running
          width: parent.width
          text: "Start Omakey"
          foreground: root.bar.foreground
          fontFamily: root.bar.fontFamily
          bordered: true
          onClicked: root.startService()
        }

        // ---- pairing ----
        Column {
          visible: root.pairing !== null
          width: parent.width
          spacing: Style.space(8)

          Rectangle {
            id: qrCard
            // QR codes need dark-on-light to scan reliably, whatever the theme.
            color: "#ffffff"
            radius: Style.space(6)
            width: Math.min(parent.width, Style.space(240))
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
            text: "Expires in " + Math.floor(root.secondsLeft / 60) + ":" + ("0" + (root.secondsLeft % 60)).slice(-2)
            textFormat: Text.PlainText
            color: Util.alpha(root.bar.foreground, 0.6)
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.caption
          }

          Button {
            width: parent.width
            text: "Cancel"
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
            onClicked: root.cancelPairing()
          }
        }

        Button {
          visible: root.running && root.pairing === null
          width: parent.width
          text: "Pair a phone"
          iconText: "󰄜"
          foreground: root.bar.foreground
          fontFamily: root.bar.fontFamily
          bordered: true
          onClicked: root.startPairing()
        }

        // ---- devices ----
        PanelSeparator {
          visible: root.devices.length > 0
          foreground: root.bar.foreground
        }

        Column {
          visible: root.devices.length > 0
          width: parent.width
          spacing: Style.space(8)

          PanelSectionHeader {
            text: "PHONES"
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
          }

          Repeater {
            model: root.devices

            Item {
              required property var modelData
              width: parent.width
              height: Math.max(info.implicitHeight, forgetButton.height)

              Rectangle {
                id: dot
                width: Style.space(7)
                height: width
                radius: width / 2
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                color: modelData.connected && root.running ? Color.accent : Util.alpha(root.bar.foreground, 0.25)
              }

              Column {
                id: info
                anchors.left: dot.right
                anchors.leftMargin: Style.space(10)
                anchors.right: forgetButton.left
                anchors.rightMargin: Style.space(8)
                anchors.verticalCenter: parent.verticalCenter

                Text {
                  width: parent.width
                  text: modelData.name
                  textFormat: Text.PlainText
                  elide: Text.ElideRight
                  color: root.bar.foreground
                  font.family: root.bar.fontFamily
                  font.pixelSize: Style.font.body
                }

                Text {
                  width: parent.width
                  text: modelData.connected && root.running
                    ? "Connected · " + (modelData.addr || "") + (modelData.held > 0 ? " · " + modelData.held + " held" : "")
                    : "Last seen " + root.ago(modelData.last_seen)
                  textFormat: Text.PlainText
                  elide: Text.ElideRight
                  color: Util.alpha(root.bar.foreground, 0.6)
                  font.family: root.bar.fontFamily
                  font.pixelSize: Style.font.caption
                }
              }

              PanelActionButton {
                id: forgetButton
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                // nf-md-link_off
                iconText: "󰌸"
                tooltipText: "Forget this phone"
                foreground: root.bar.foreground
                enabled: root.running
                onClicked: root.forget(modelData.id)
              }
            }
          }
        }

        Text {
          visible: root.running
          width: parent.width
          text: (state.addresses || []).join(", ") + " · UDP " + (state.port || "")
          textFormat: Text.PlainText
          elide: Text.ElideRight
          color: Util.alpha(root.bar.foreground, 0.45)
          font.family: root.bar.fontFamily
          font.pixelSize: Style.font.caption
        }
      }
    }
  }
}
