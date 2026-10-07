import QtQuick
import QtQuick.Shapes

// Shared with the Android launcher icon, with colors supplied by the bar.
Item {
  id: root
  property color color: "white"
  implicitWidth: 24
  implicitHeight: 24

  Shape {
    width: 24
    height: 24
    anchors.centerIn: parent
    scale: Math.min(root.width, root.height) / 24
    preferredRendererType: Shape.CurveRenderer

    ShapePath {
      fillColor: "transparent"
      strokeColor: root.color
      strokeWidth: 2
      capStyle: ShapePath.RoundCap
      joinStyle: ShapePath.RoundJoin
      PathSvg { path: "M11.5,4 H7 C4.79,4 3,5.79 3,8 V17 C3,19.21 4.79,21 7,21 H16 C18.21,21 20,19.21 20,17 V13" }
    }

    ShapePath {
      fillColor: "transparent"
      strokeColor: root.color
      strokeWidth: 2
      capStyle: ShapePath.RoundCap
      joinStyle: ShapePath.RoundJoin
      PathSvg { path: "M15,11.5 V14.5 H8 M10.5,12 L8,14.5 L10.5,17" }
    }

    ShapePath {
      fillColor: "transparent"
      strokeColor: root.color
      strokeWidth: 2
      capStyle: ShapePath.RoundCap
      PathSvg { path: "M16,3 C19.31,3 22,5.69 22,9 M16,7 C17.1,7 18,7.9 18,9" }
    }
  }
}
