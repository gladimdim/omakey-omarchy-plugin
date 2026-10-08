import QtQuick
import QtQuick.Shapes

// The Android launcher icon: the gradient keycap and arrow from
// OmakeyIcon.svg, and the signal waves, in mint while [lit] (a phone is
// connected) and in [color] otherwise.
Item {
  id: root
  property color color: "white"
  property bool lit: true
  implicitWidth: 24
  implicitHeight: 24

  readonly property real side: Math.min(width, height)

  Image {
    anchors.centerIn: parent
    width: root.side
    height: root.side
    source: Qt.resolvedUrl("OmakeyIcon.svg")
    // Rasterized at the size shown, and again for HiDPI, so it stays crisp.
    sourceSize.width: Math.ceil(root.side * 2)
    sourceSize.height: Math.ceil(root.side * 2)
    smooth: true
    mipmap: true
  }

  Shape {
    width: 24
    height: 24
    anchors.centerIn: parent
    scale: root.side / 24
    preferredRendererType: Shape.CurveRenderer

    ShapePath {
      fillColor: "transparent"
      strokeColor: root.lit ? "#B2FFE0" : root.color
      strokeWidth: 2
      capStyle: ShapePath.RoundCap
      PathSvg { path: "M16,3 C19.31,3 22,5.69 22,9 M16,7 C17.1,7 18,7.9 18,9" }
    }
  }
}
